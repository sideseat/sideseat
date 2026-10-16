//! Agent Framework 1.0.1 streams a run whose chat spans are the invoke_agent span's siblings: the claim holds there,
//! and each clause of it, broken alone, refuses it - and with it the attribution the claim allows.

use super::*;
use crate::message_truth::mutations::{added, baseline};
use crate::message_truth::recon::Recon;

const UNNESTED: &str = "agent-framework/native@1.0.1/streaming";
const NESTED: &str = "agent-framework/native/streaming";
const CALLS: [&str; 2] = ["call-001", "call-002"];

fn matched(truth: &Truth, recon: &Recon) -> Matching {
    super::super::matching::match_calls(truth, recon, &mut Vec::new())
}

fn agent(recon: &Recon) -> usize {
    let runs: Vec<usize> = (0..recon.generations.len())
        .filter(|&i| recon.generations[i].agent)
        .collect();
    assert_eq!(runs.len(), 1, "one agent span in the capture");
    runs[0]
}

fn call_span(truth: &Truth, recon: &Recon, call: &str) -> usize {
    matched(truth, recon).span_of[call]
}

/// Every call's claim is refused, the attribution it allowed is owed again, and nothing is adopted.
fn refused(truth: &Truth, recon: &Recon, before: &std::collections::BTreeSet<String>, what: &str) {
    let matching = matched(truth, recon);
    for call in CALLS {
        assert!(
            run_span(truth, call, recon, &matching).is_err(),
            "{what}: {call}'s claim was not refused"
        );
    }
    assert!(
        Runs::of(truth, recon, &matching).is_empty(),
        "{what}: a run was adopted"
    );
    let new = added(truth, recon, before);
    assert!(
        new.contains("attribution.span:fact-001"),
        "{what}: the prompt on the run's span was not caught: {new:?}"
    );
}

#[test]
fn a_call_beside_its_run_is_proven_and_the_run_adopted() {
    let (truth, recon, _) = baseline(UNNESTED);
    let matching = matched(&truth, &recon);
    let run = agent(&recon);
    for call in CALLS {
        assert_eq!(
            prove_unnested(&truth, call, &recon, &matching),
            Proof::Absent
        );
        assert_eq!(run_span(&truth, call, &recon, &matching), Ok(run));
    }
    let runs = Runs::of(&truth, &recon, &matching);
    let run = &recon.generations[run];
    for call in CALLS {
        assert!(runs.of_call(call, &run.trace, &run.span));
    }
    // The prompt's home is the first call: no recorded request re-sends it, so it is held to that call's span or
    // one enclosing it, the run's among them.
    assert!(super::super::requests::sent_to(&truth, &recon).is_empty());
}

#[test]
fn each_clause_broken_alone_refuses_the_claim() {
    let (truth, recon, before) = baseline(UNNESTED);
    let run = agent(&recon);
    let calls: Vec<usize> = CALLS.iter().map(|c| call_span(&truth, &recon, c)).collect();

    // Another parent: the run is no sibling of the calls.
    let mut moved = recon.clone();
    moved.generations[run].ancestors = vec!["another-parent".into()];
    refused(&truth, &moved, &before, "a run under another parent");

    // Another trace: the run is in no trace of the calls.
    let mut traced = recon.clone();
    traced.generations[run].trace = "another-trace".into();
    refused(&truth, &traced, &before, "a run in another trace");

    // A window too short: the run ends before its last call does, or records no end.
    let mut short = recon.clone();
    short.generations[run].end = Some(recon.generations[calls[0]].start);
    refused(&truth, &short, &before, "a run ending before its calls");
    let mut open = recon.clone();
    open.generations[run].end = None;
    refused(&truth, &open, &before, "a run with no end");
    let mut late = recon.clone();
    late.generations[run].start =
        recon.generations[calls[1]].end.expect("the call ends") + chrono::Duration::nanoseconds(1);
    late.generations[run].end = late.generations[run]
        .start
        .checked_add_signed(chrono::Duration::seconds(1));
    refused(&truth, &late, &before, "a run starting after its calls");

    // A second agent span beside the calls: two runs they could belong to.
    let mut twice = recon.clone();
    let mut second = recon.generations[run].clone();
    second.span = "second-run".into();
    second.label = format!("{}/second", second.label);
    twice.generations.push(second.clone());
    refused(&truth, &twice, &before, "a second run beside the calls");

    // Another agent span elsewhere in the trace lasting as long as the calls.
    let mut elsewhere = recon.clone();
    second.ancestors = vec!["another-parent".into()];
    elsewhere.generations.push(second);
    refused(
        &truth,
        &elsewhere,
        &before,
        "another run holding the calls' window",
    );

    // The calls not typed model calls by the reading.
    let mut untyped = recon.clone();
    for &at in &calls {
        untyped.generations[at].typed = false;
    }
    let matching = matched(&truth, &untyped);
    for call in CALLS {
        assert!(
            run_span(&truth, call, &untyped, &matching).is_err(),
            "an untyped {call}"
        );
    }
    assert!(Runs::of(&truth, &untyped, &matching).is_empty());
}

#[test]
fn a_call_nested_under_its_run_is_refused() {
    // The mutant: the 1.0.1 calls placed under the run, as a parent.
    let (truth, recon, _) = baseline(UNNESTED);
    let run = agent(&recon);
    let mut nested = recon.clone();
    for call in CALLS {
        let at = call_span(&truth, &recon, call);
        nested.generations[at]
            .ancestors
            .insert(0, recon.generations[run].span.clone());
    }
    let matching = matched(&truth, &nested);
    for call in CALLS {
        assert!(matches!(
            run_span(&truth, call, &nested, &matching),
            Err(Proof::Present(_))
        ));
    }
    // The current release, which nests its calls, with the claim forced onto it.
    let (mut current, recon, _) = baseline(NESTED);
    let truths = super::super::truth::load_all();
    current.gaps.extend(
        truths["agent-framework/streaming"]
            .gaps
            .iter()
            .filter(|g| g.reason == REASON)
            .cloned(),
    );
    let matching = matched(&current, &recon);
    for call in CALLS {
        assert!(
            matches!(
                run_span(&current, call, &recon, &matching),
                Err(Proof::Present(_))
            ),
            "{call} of the current release is nested under its run"
        );
    }
    assert!(Runs::of(&current, &recon, &matching).is_empty());
}

#[test]
fn only_the_run_s_span_is_adopted() {
    // The prompt shown on the tool span beside the calls, rather than on the run's: still misattributed.
    let (truth, recon, before) = baseline(UNNESTED);
    let run = agent(&recon);
    let parent = recon.generations[run].ancestors.first().cloned();
    let tool = recon
        .generations
        .iter()
        .find(|g| !g.agent && !g.typed && g.ancestors.first() == parent.as_ref())
        .map(|g| g.span.clone())
        .expect("the tool span beside the calls");
    let prompt = truth
        .facts
        .iter()
        .find(|f| f.id == "fact-001")
        .expect("the prompt");
    let mut elsewhere = recon.clone();
    for (v, b) in super::super::mutate::locate(prompt, &elsewhere) {
        if elsewhere.views[v].kind != super::super::recon::ViewKind::Span
            && elsewhere.views[v].blocks[b].span == recon.generations[run].span
        {
            elsewhere.views[v].blocks[b].span = tool.clone();
        }
    }
    let new = added(&truth, &elsewhere, &before);
    assert!(
        new.contains("attribution.span:fact-001"),
        "the prompt on the tool span was not caught: {new:?}"
    );
}

#[test]
fn a_run_is_adopted_in_its_calls_trace_only() {
    // Span ids repeat across traces: a span of another trace reusing the run's id is not the run the calls were
    // recorded beside, nor is any span but the run's, nor the run of a call that was never proven.
    let (truth, recon, _) = baseline(UNNESTED);
    let matching = matched(&truth, &recon);
    let runs = Runs::of(&truth, &recon, &matching);
    let run = &recon.generations[agent(&recon)];
    let call = &recon.generations[matching.span_of["call-001"]];
    assert!(runs.of_call("call-001", &run.trace, &run.span));
    assert!(!runs.of_call("call-001", "another-trace", &run.span));
    assert!(!runs.of_call("call-001", &run.trace, &call.span));
    assert!(!runs.of_call("call-003", &run.trace, &run.span));
    // A fact sent only to a call no gap names is at home on no run.
    let mut unclaimed = truth.clone();
    unclaimed.gaps.retain(|g| g.reason != REASON);
    assert!(Runs::of(&unclaimed, &recon, &matching).is_empty());
}
