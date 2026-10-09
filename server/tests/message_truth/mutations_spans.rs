//! Mutations for two producer limitations the truths declare per call: a call recorded on the run's span
//! (`call_span_not_exported`) and a call the producer started a new trace for (`trace_not_propagated`). Each
//! frees the call from what the producer never wrote - a span of its own, one trace - and from nothing else:
//! every defect the views could still show is still caught.

use super::mutate::{locate, remove_positions};
use super::mutations::{added, baseline};
use super::predicates::{Shows, shows};
use super::recon::{Block, Recon, ViewKind};
use super::truth::{Fact, Truth};

fn fact<'t>(truth: &'t Truth, id: &str) -> &'t Fact {
    truth.facts.iter().find(|f| f.id == id).expect("a fact")
}

fn first_shown(blocks: &[Block], fact: &Fact) -> Option<usize> {
    blocks
        .iter()
        .position(|b| shows(fact, b, None) != Shows::No)
}

/// Moves the first block showing `moved` in front of the first showing `before`, in every view of `kind`.
fn move_before(recon: &mut Recon, kind: ViewKind, moved: &Fact, before: &Fact) {
    for view in recon.views.iter_mut().filter(|v| v.kind == kind) {
        if let (Some(a), Some(c)) = (
            first_shown(&view.blocks, moved),
            first_shown(&view.blocks, before),
        ) {
            let block = view.blocks.remove(a);
            let at = if a < c { c - 1 } else { c };
            view.blocks.insert(at, block);
        }
    }
}

fn caught(new: &std::collections::BTreeSet<String>, what: &str, test: impl Fn(&str) -> bool) {
    assert!(
        new.iter().any(|v| test(v)),
        "{what} was not caught: {new:?}"
    );
}

#[test]
fn a_call_recorded_on_the_run_s_span_still_owes_its_parts_in_every_view() {
    // CrewAI records no model-call span: the call answering the tool round is on the run's agent span. That
    // frees it from a span of its own, not from the views, the span it shares, or their order.
    let (truth, recon, baseline) = baseline("crewai/sdk/tool_use");
    let (answer, first_call) = (fact(&truth, "fact-010"), fact(&truth, "fact-002"));
    let mut missing = recon.clone();
    let positions: Vec<(usize, usize)> = locate(answer, &missing)
        .into_iter()
        .filter(|&(v, _)| missing.views[v].kind == ViewKind::Trace)
        .collect();
    assert!(!positions.is_empty(), "the trace view shows the answer");
    remove_positions(&mut missing, positions);
    caught(
        &added(&truth, &missing, &baseline),
        "the answer missing from the trace view",
        |v| v.ends_with(":fact-010") && v.contains(".missing"),
    );
    for kind in [ViewKind::Trace, ViewKind::Feed] {
        // The feed lists one span's calls oldest first, as the span does: the answer before its tool calls is
        // out of order there too.
        let mut early = recon.clone();
        let (moved, before) = if kind == ViewKind::Feed {
            (first_call, answer)
        } else {
            (answer, first_call)
        };
        move_before(&mut early, kind, moved, before);
        caught(
            &added(&truth, &early, &baseline),
            &format!("the answer out of order in the {kind:?} view"),
            |v| v.starts_with("order."),
        );
    }
    // Shown as another span's - a tool span's - in the conversation views: attributed to the wrong span.
    let mut elsewhere = recon.clone();
    let shared = recon.generations[elsewhere_span(&truth, &recon)]
        .span
        .clone();
    let sibling = recon
        .generations
        .iter()
        .find(|g| g.ancestors.first() == Some(&shared))
        .map(|g| g.span.clone())
        .expect("a span within the run");
    for (v, b) in locate(answer, &elsewhere) {
        if elsewhere.views[v].kind != ViewKind::Span {
            elsewhere.views[v].blocks[b].span = sibling.clone();
        }
    }
    caught(
        &added(&truth, &elsewhere, &baseline),
        "the answer shown on a tool span",
        |v| v == "attribution.span:fact-010",
    );
}

/// The run's span: the one the first call is tied to.
fn elsewhere_span(truth: &Truth, recon: &Recon) -> usize {
    let matching = super::matching::match_calls(truth, recon, &mut Vec::new());
    matching.span_of["call-001"]
}

#[test]
fn a_trace_the_producer_split_off_explains_only_its_own_copies() {
    // Spring AI starts a new trace for the call answering a streamed tool round: what that call was sent is at
    // home there too, once, and the session still owes what the conversation's own trace carries.
    let (truth, recon, baseline) = baseline("spring-ai/sdk/streaming");
    let (prompt, result) = (fact(&truth, "fact-001"), fact(&truth, "fact-005"));
    let moved = recon
        .generations
        .iter()
        .find(|g| g.ancestors.is_empty() && g.typed)
        .map(|g| g.trace.clone())
        .expect("the split-off call's span");
    let in_view =
        |recon: &Recon, fact: &Fact, kind: ViewKind, trace: &str| -> Vec<(usize, usize)> {
            locate(fact, recon)
                .into_iter()
                .filter(|&(v, b)| {
                    recon.views[v].kind == kind && recon.views[v].blocks[b].trace == trace
                })
                .collect()
        };
    // The prompt in a trace nothing sent it: a leak.
    let mut leaked = recon.clone();
    let &(v, b) = locate(prompt, &leaked)
        .iter()
        .find(|&&(v, _)| leaked.views[v].kind == ViewKind::Feed)
        .expect("the feed shows the prompt");
    let mut copy = leaked.views[v].blocks[b].clone();
    copy.trace = "an-unrelated-trace".into();
    copy.span = "an-unrelated-span".into();
    leaked.views[v].blocks.insert(0, copy);
    caught(
        &added(&truth, &leaked, &baseline),
        "the prompt in an unrelated trace",
        |v| v.ends_with(":fact-001"),
    );
    // A second copy in the split-off trace, of the prompt or of the result only that trace carries.
    for fact in [prompt, result] {
        let mut twice = recon.clone();
        let &(v, b) = in_view(&twice, fact, ViewKind::Trace, &moved)
            .first()
            .expect("the split-off trace shows it");
        let copy = twice.views[v].blocks[b].clone();
        twice.views[v].blocks.insert(b + 1, copy);
        caught(
            &added(&truth, &twice, &baseline),
            "a second copy in the split-off trace",
            |v| v.contains(&fact.id) && v.contains(".duplicated"),
        );
    }
    // The split-off trace without the prompt it was sent: missing there.
    let mut dropped = recon.clone();
    let positions = in_view(&dropped, prompt, ViewKind::Trace, &moved);
    assert!(!positions.is_empty());
    remove_positions(&mut dropped, positions);
    caught(
        &added(&truth, &dropped, &baseline),
        "the prompt missing from the split-off trace",
        |v| v.contains("fact-001@") && v.contains(".missing"),
    );
    // The conversation's own trace and the session without the prompt, the split-off trace keeping it: the
    // session still owes it, since a raw carrier of the session's trace holds it.
    let home = recon
        .generations
        .iter()
        .find(|g| g.typed && g.trace != moved)
        .map(|g| g.trace.clone())
        .expect("the conversation's trace");
    let mut lost = recon.clone();
    let mut positions = in_view(&lost, prompt, ViewKind::Trace, &home);
    positions.extend(
        locate(prompt, &lost)
            .into_iter()
            .filter(|&(v, _)| lost.views[v].kind == ViewKind::Session),
    );
    remove_positions(&mut lost, positions);
    caught(
        &added(&truth, &lost, &baseline),
        "the prompt lost from the session",
        |v| v.starts_with("user_text.missing:fact-001"),
    );
    // The session's prompt replaced by the split-off trace's copy, a trace no session holds: not at home.
    let mut swapped = recon.clone();
    for view in swapped
        .views
        .iter_mut()
        .filter(|v| v.kind == ViewKind::Session)
    {
        if let Some(at) = first_shown(&view.blocks, prompt) {
            view.blocks[at].trace = moved.clone();
        }
    }
    caught(
        &added(&truth, &swapped, &baseline),
        "the session's prompt from the split-off trace",
        |v| v.ends_with(":fact-001"),
    );
}

#[test]
fn the_run_s_span_owes_a_shared_call_s_parts_once_and_in_order() {
    // The agent span CrewAI records the whole run on shows the call answering the tool round: duplicated, or
    // ahead of the tool calls it answers, in that span's own view only, it is still caught.
    let (truth, recon, baseline) = baseline("crewai/sdk/tool_use");
    let (answer, first_call) = (fact(&truth, "fact-010"), fact(&truth, "fact-002"));
    let run = recon.generations[elsewhere_span(&truth, &recon)]
        .span
        .clone();
    let mut twice = recon.clone();
    let &(v, b) = locate(answer, &twice)
        .iter()
        .find(|&&(v, _)| twice.views[v].kind == ViewKind::Span && twice.views[v].key == run)
        .expect("the run's span view shows the answer");
    let copy = twice.views[v].blocks[b].clone();
    twice.views[v].blocks.insert(b + 1, copy);
    caught(
        &added(&truth, &twice, &baseline),
        "the answer twice on the run's span",
        |v| v.starts_with("text.duplicated:fact-010"),
    );
    let mut early = recon.clone();
    for view in early
        .views
        .iter_mut()
        .filter(|v| v.kind == ViewKind::Span && v.key == run)
    {
        if let (Some(a), Some(c)) = (
            first_shown(&view.blocks, answer),
            first_shown(&view.blocks, first_call),
        ) {
            let block = view.blocks.remove(a);
            view.blocks.insert(c, block);
        }
    }
    caught(
        &added(&truth, &early, &baseline),
        "the answer ahead of its tool calls on the run's span",
        |v| v.starts_with("order."),
    );
}

#[test]
fn a_split_off_trace_is_checked_as_a_view_of_its_own() {
    // Spring AI's split-off trace owes what its call was sent in order and on that call's span; the
    // conversation's own trace still owes everything, even when its whole view is lost.
    let (truth, recon, baseline) = baseline("spring-ai/sdk/streaming");
    let (prompt, call_a) = (fact(&truth, "fact-001"), fact(&truth, "fact-003"));
    let result = fact(&truth, "fact-005");
    let moved = recon
        .generations
        .iter()
        .find(|g| g.ancestors.is_empty() && g.typed)
        .map(|g| g.trace.clone())
        .expect("the split-off call's span");
    let in_moved = |recon: &mut Recon, edit: &dyn Fn(&mut Vec<Block>)| {
        for view in recon.views.iter_mut().filter(|v| v.kind == ViewKind::Trace) {
            if view.key == moved {
                edit(&mut view.blocks);
            }
        }
    };
    // The conversation's trace view emptied: its facts are missing there, whatever the split-off trace shows.
    let mut lost = recon.clone();
    for view in lost
        .views
        .iter_mut()
        .filter(|v| v.kind == ViewKind::Trace && v.key != moved)
    {
        view.blocks.clear();
    }
    caught(
        &added(&truth, &lost, &baseline),
        "the conversation's trace view lost",
        |v| v.starts_with("user_text.missing:fact-001") && !v.contains('@'),
    );
    // The split-off trace's tool calls after the result they produced.
    let mut late = recon.clone();
    in_moved(&mut late, &|blocks| {
        if let (Some(c), Some(r)) = (first_shown(blocks, call_a), first_shown(blocks, result)) {
            let block = blocks.remove(c);
            blocks.insert(r, block);
        }
    });
    caught(
        &added(&truth, &late, &baseline),
        "a call after its result in the split-off trace",
        |v| v.starts_with("order.") && v.ends_with("@trace-2"),
    );
    // The split-off trace's prompt shown on another span than its call's.
    let mut elsewhere = recon.clone();
    in_moved(&mut elsewhere, &|blocks| {
        if let Some(at) = first_shown(blocks, prompt) {
            blocks[at].span = "another-span".into();
        }
    });
    caught(
        &added(&truth, &elsewhere, &baseline),
        "the prompt on another span in the split-off trace",
        |v| v == "attribution.span:fact-001@trace-2",
    );
    // The conversation's first tool call attributed to the run's root span, which never produced it.
    let root = recon
        .generations
        .iter()
        .find(|g| !g.typed && g.ancestors.is_empty())
        .map(|g| g.span.clone())
        .expect("the run's root span");
    let mut parent = recon.clone();
    for (v, b) in locate(call_a, &parent) {
        if parent.views[v].kind == ViewKind::Trace && parent.views[v].blocks[b].trace != moved {
            parent.views[v].blocks[b].span = root.clone();
        }
    }
    caught(
        &added(&truth, &parent, &baseline),
        "a tool call on the run's root span",
        |v| v == "attribution.span:fact-003",
    );
}
