//! A model call recorded on no span of its own: the producer writes no model-call span at all, and the span
//! of the call before it - an agent span holding the whole run - carries this call's output too.

use super::Proof;
use crate::message_truth::matching::{Matching, outputs, shown_in, signature};
use crate::message_truth::recon::Recon;
use crate::message_truth::truth::Truth;

/// `call_span_not_exported` holds where no span of the capture is a model call - had the producer written one,
/// the call could have its own - the span the previous call of the conversation is tied to shows every asserted
/// output of this call as well, and no other span but one enclosing it shows any of it as its own output.
/// Anything else is refused: a model-call span anywhere, another span with the output, a first call, a call
/// with nothing asserted, or a shared span that does not carry all of the output.
pub(super) fn prove_call_span(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
) -> Proof {
    if let Some(generation) = recon.generations.iter().find(|g| g.typed) {
        return Proof::Present(format!("{} is a model-call span", generation.label));
    }
    let Some(position) = truth.calls.iter().position(|c| c.id == call_id) else {
        return Proof::Unprovable(format!("no call {call_id}"));
    };
    let call = &truth.calls[position];
    // The nearest earlier call that is tied to a span: a call between them may share that span too.
    let earlier: Vec<_> = truth.calls[..position]
        .iter()
        .rev()
        .filter(|c| c.conversation == call.conversation && c.succeeded())
        .collect();
    if earlier.is_empty() {
        return Proof::Unprovable(
            "the first call of its conversation has no span to share".to_string(),
        );
    }
    let Some(&span) = earlier.iter().find_map(|c| matching.span_of.get(&c.id)) else {
        return Proof::Unprovable(
            "no earlier call of its conversation is tied to a span".to_string(),
        );
    };
    let shared = outputs(recon, &recon.generations[span]);
    let owed = signature(truth, call);
    if owed.is_empty() {
        return Proof::Unprovable(format!("{call_id} has no asserted output to look for"));
    }
    // A span of its own would show its output as its own: then the call has a span, typed or not. A span
    // enclosing the run's - a crew reporting its result - restates the run's output and is not one.
    let enclosing = &recon.generations[span].ancestors;
    if let Some(own) = recon
        .generations
        .iter()
        .enumerate()
        .filter(|&(g, other)| g != span && !enclosing.contains(&other.span))
        .find(|(_, g)| {
            let shown = outputs(recon, g);
            owed.iter().any(|fact| shown_in(fact, &shown))
        })
    {
        return Proof::Present(format!("{} shows this call's output", own.1.label));
    }
    match owed.iter().find(|fact| !shown_in(fact, &shared)) {
        None => Proof::Absent,
        Some(fact) => Proof::Unprovable(format!(
            "{} does not show {}, an output of this call",
            recon.generations[span].label, fact.id
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a capture with no model-call span the call answering a tool round shares the run's span, and that is
    /// proven; a model-call span anywhere, another span showing the output, a first call, or a shared span
    /// missing one output refuses it.
    #[test]
    fn a_call_shares_the_run_s_span_only_where_no_model_call_span_exists() {
        let fixture = "crewai/sdk/tool_use";
        let paths = crate::discover_fixtures()
            .into_iter()
            .find(|(label, _)| label == fixture)
            .map(|(_, paths)| paths)
            .expect("the capture");
        let truths = crate::message_truth::truth::load_all();
        let truth = truths["crewai/tool_use"].for_fixture(fixture);
        let recon = crate::message_truth::recon::build(fixture, &paths);
        let matching = crate::message_truth::matching::match_calls(&truth, &recon, &mut Vec::new());
        assert_eq!(
            prove_call_span(&truth, "call-002", &recon, &matching),
            Proof::Absent
        );
        assert!(matches!(
            prove_call_span(&truth, "call-001", &recon, &matching),
            Proof::Unprovable(_)
        ));
        // The run's span without one of the call's outputs: the call is then missing, not sharing it.
        let mut cut = recon.clone();
        let shared = cut.generations[matching.span_of["call-001"]].span.clone();
        let last = *signature(&truth, &truth.calls[1])
            .last()
            .expect("an output");
        for view in cut
            .views
            .iter_mut()
            .filter(|v| v.kind == crate::message_truth::recon::ViewKind::Span && v.key == shared)
        {
            view.blocks.retain(|b| !(b.output && shown_in(last, &[b])));
        }
        assert!(matches!(
            prove_call_span(&truth, "call-002", &cut, &matching),
            Proof::Unprovable(_)
        ));
        // Another span showing the call's output is a span of its own, typed or not; one enclosing the run's
        // span restating it is not.
        let mut own = recon.clone();
        let enclosing = own.generations[matching.span_of["call-001"]]
            .ancestors
            .clone();
        let other = own
            .generations
            .iter()
            .find(|g| g.span != shared && !enclosing.contains(&g.span))
            .map(|g| g.span.clone())
            .expect("a second span");
        let copy = own
            .span_view(&shared)
            .and_then(|v| v.blocks.iter().find(|b| b.output && shown_in(last, &[b])))
            .cloned()
            .expect("the shared span shows the output");
        for view in own
            .views
            .iter_mut()
            .filter(|v| v.kind == crate::message_truth::recon::ViewKind::Span && v.key == other)
        {
            view.blocks.push(copy.clone());
        }
        assert!(matches!(
            prove_call_span(&truth, "call-002", &own, &matching),
            Proof::Present(_)
        ));
        let mut typed = recon.clone();
        typed.generations[0].typed = true;
        assert!(matches!(
            prove_call_span(&truth, "call-002", &typed, &matching),
            Proof::Present(_)
        ));
    }
}
