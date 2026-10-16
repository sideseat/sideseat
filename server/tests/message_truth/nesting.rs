//! A model call its producer records beside the agent run that made it rather than inside it
//! (`call_not_nested_under_its_run`): the run's span is opened without becoming the current span, so the calls the
//! run makes open under the run's own parent, as its siblings. What a call was sent is then shown on the run's span,
//! which the reading attributes a run's input to, though that span is not an ancestor of the call's. The run's span
//! is accepted as the call's enclosing one, and nothing else is: the claim names exactly one span per call.

use std::collections::BTreeMap;

use super::absence::Proof;
use super::matching::Matching;
use super::recon::{Generation, Recon};
use super::truth::Truth;

pub(super) const REASON: &str = "call_not_nested_under_its_run";

/// Whether `outer`'s recorded window holds `inner`'s: a run lasts as long as every call it makes. A span with no
/// recorded end holds nothing, and is held by nothing.
fn encloses(outer: &Generation, inner: &Generation) -> bool {
    match (outer.end, inner.end) {
        (Some(outer_end), Some(inner_end)) => outer.start <= inner.start && inner_end <= outer_end,
        _ => false,
    }
}

/// The run span a call's span sits beside, as a generation index, where the claim holds: the call is tied to a span
/// the reading types a model call; no span enclosing it is typed an agent's run, as a nested call's would be; that
/// span has a parent; exactly one span typed an agent's run shares the trace and that parent; it holds the call's
/// window; and no other agent span of the trace does - there is one run the call can belong to. Anything else is
/// refused, with the clause it fails.
pub(super) fn run_span(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
) -> Result<usize, Proof> {
    let refuse = |why: String| Err(Proof::Unprovable(why));
    if !truth.calls.iter().any(|c| c.id == call_id) {
        return refuse(format!("no call {call_id}"));
    }
    let Some(&at) = matching.span_of.get(call_id) else {
        return refuse(format!("{call_id} is tied to no span"));
    };
    let call = &recon.generations[at];
    if !call.typed || call.agent {
        return refuse(format!("{} is not typed a model call", call.label));
    }
    let in_trace = |g: &&Generation| g.trace == call.trace && g.span != call.span;
    if let Some(run) = recon
        .generations
        .iter()
        .filter(in_trace)
        .find(|g| g.agent && call.ancestors.contains(&g.span))
    {
        return Err(Proof::Present(format!(
            "{} is nested under {}, an agent span",
            call.label, run.label
        )));
    }
    let Some(parent) = call.ancestors.first() else {
        return refuse(format!("{} has no parent span", call.label));
    };
    let beside: Vec<usize> = recon
        .generations
        .iter()
        .enumerate()
        .filter(|(_, g)| in_trace(g) && g.agent && g.ancestors.first() == Some(parent))
        .map(|(i, _)| i)
        .collect();
    let [run] = beside[..] else {
        return refuse(format!(
            "{} agent spans share {}'s parent; one run is claimed",
            beside.len(),
            call.label
        ));
    };
    if !encloses(&recon.generations[run], call) {
        return refuse(format!(
            "{} does not last as long as {}",
            recon.generations[run].label, call.label
        ));
    }
    if let Some(other) = recon
        .generations
        .iter()
        .enumerate()
        .find(|&(i, g)| i != run && in_trace(&g) && g.agent && encloses(g, call))
    {
        return refuse(format!(
            "{} also holds {}'s window",
            other.1.label, call.label
        ));
    }
    Ok(run)
}

/// `call_not_nested_under_its_run` holds where `run_span` names the run.
pub(super) fn prove_unnested(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
) -> Proof {
    run_span(truth, call_id, recon, matching).map_or_else(|refused| refused, |_| Proof::Absent)
}

/// The runs proven calls were recorded beside, each by its trace and span: span ids repeat across traces, so a
/// span of another trace reusing an id is no run of these calls. Only a call's own home is adopted: a fact a
/// recorded request re-sent is held to the spans of the calls that were sent it, as before.
#[derive(Debug, Default)]
pub(super) struct Runs(BTreeMap<String, (String, String)>);

impl Runs {
    /// Every call a `call_not_nested_under_its_run` gap names and `run_span` proves, with its run.
    pub(super) fn of(truth: &Truth, recon: &Recon, matching: &Matching) -> Self {
        Self(
            truth
                .gaps
                .iter()
                .filter(|g| g.reason == REASON)
                .filter_map(|g| g.subject.as_deref())
                .filter_map(|call| {
                    let run = &recon.generations[run_span(truth, call, recon, matching).ok()?];
                    Some((call.to_string(), (run.trace.clone(), run.span.clone())))
                })
                .collect(),
        )
    }

    /// Whether the span `span` of `trace` is the run `call` was recorded beside: what it was sent may be shown there,
    /// as on a span enclosing the call's.
    pub(super) fn of_call(&self, call: &str, trace: &str, span: &str) -> bool {
        self.0
            .get(call)
            .is_some_and(|(t, s)| t == trace && s == span)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests;
