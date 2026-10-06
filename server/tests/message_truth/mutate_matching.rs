//! Mutations of where a reconstruction places a call relative to the span that recorded it.

use super::mutate::call_facts;
use super::predicates::{Shows, shows};
use super::recon::{Generation, Recon, ViewKind};
use super::truth::{Fact, Truth};

/// A call's output shown in the trace under a span below the one that recorded it, with that span's
/// usage corrupted: the matcher no longer takes the recording span as the call's, and the call must
/// still fail rather than escape its usage check.
pub(super) fn relist_below_and_corrupt_usage(truth: &mut Truth, recon: &mut Recon) -> bool {
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    let Some((call, &span)) = matching.span_of.iter().next() else {
        return false;
    };
    let facts: Vec<Fact> = call_facts(truth, call).into_iter().cloned().collect();
    let parent = recon.generations[span].clone();
    let child = format!("{}-below", parent.span);
    let mut moved = false;
    for view in recon.views.iter_mut().filter(|v| v.kind == ViewKind::Trace) {
        for block in &mut view.blocks {
            if facts.iter().any(|f| shows(f, block, None) != Shows::No) {
                block.span = child.clone();
                moved = true;
            }
        }
    }
    let mut ancestors = vec![parent.span.clone()];
    ancestors.extend(parent.ancestors.iter().cloned());
    recon.generations.push(Generation {
        span: child.clone(),
        label: format!("{}/below", parent.label),
        typed: false,
        ancestors,
        response_id: None,
        ..parent
    });
    recon.generations[span].input += 1000;
    recon.generations[span].output += 1000;
    moved
}
