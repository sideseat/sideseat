//! What a composed request's view may show, for a producer that exports each request as what it added.
//!
//! Such a span's view holds more than its own span carried: the requests of its thread before it, and the tool
//! spans whose calls those requests answered (`docs/engineering/request-context.md`). `check_requests` keeps
//! owing the whole request whatever the composition reports, and these two checks bound what the composition may
//! use to pay that debt.
//!
//! - **Occurrence provenance.** Every block a composed view shows that is not the request's own must be a
//!   specific *occurrence* - a span, a carrier and a position - of a request of the same thread or of a tool span
//!   that thread owns, and the composed blocks must keep the thread's order. Content identity is not enough: a
//!   block carrying the right text from the wrong occurrence is a composition that found its answer by accident,
//!   and nothing downstream would tell the two apart.
//! - **Thread isolation.** A block that only another thread carried has no business in this request, whatever it
//!   says. A subagent's thread and its parent's are different conversations, and the key tells them apart.
//!
//! Neither check knows a producer: a view is composed when the harness says its scope is, and what a thread is
//! comes from the rules' own declaration.

use std::collections::{BTreeMap, BTreeSet};

use super::recon::{Recon, ViewKind};
use super::{Violation, ViolationView};

/// Where a block came from, which is what "one occurrence" means.
type Occurrence = (String, String, String, String);

fn occurrence(block: &super::recon::Block) -> Occurrence {
    (
        block.trace.clone(),
        block.span.clone(),
        block.carrier.clone(),
        block.position.clone(),
    )
}

/// Check every composed span view of this fixture.
pub(super) fn check_composition(recon: &Recon, out: &mut Vec<Violation>) {
    // Every occurrence each span offers, so a composed block can be traced to the span it claims. A span's **own**
    // view and its own blocks in it: a composed copy is not evidence of itself, and taking one as evidence would
    // make a forged carrier its own witness.
    let mut offered: BTreeMap<(String, String), Vec<Occurrence>> = BTreeMap::new();
    for view in recon.views.iter().filter(|v| v.kind == ViewKind::Span) {
        for block in view.blocks.iter().filter(|block| block.span == view.key) {
            offered
                .entry((block.trace.clone(), block.span.clone()))
                .or_default()
                .push(occurrence(block));
        }
    }

    for view in recon
        .views
        .iter()
        .filter(|v| v.kind == ViewKind::Span && !v.thread.is_empty())
    {
        let own: BTreeSet<(String, String)> = view
            .blocks
            .iter()
            .filter(|block| block.span == view.key)
            .map(|block| (block.trace.clone(), block.span.clone()))
            .collect();
        // The spans this view may draw on: itself, its thread, and the tool spans it owns.
        let allowed: BTreeSet<(String, String)> = view
            .thread
            .iter()
            .cloned()
            .chain(view.owned_calls.iter().cloned())
            .chain(own)
            .collect();
        // The thread's own order, so composed blocks can be held to it. The view's **own** span is left out: its
        // frame precedes the history and its delta follows it, so ranking it would make every composed block
        // read as out of order.
        let rank: BTreeMap<(String, String), usize> = view
            .thread
            .iter()
            .filter(|(_, span)| span != &view.key)
            .enumerate()
            .map(|(at, span)| (span.clone(), at))
            .collect();

        let mut last_rank = 0usize;
        for (index, block) in view.blocks.iter().enumerate() {
            let origin = (block.trace.clone(), block.span.clone());
            let subject = format!("{}:{index}", view.key);
            if !allowed.contains(&origin) {
                // A span outside the thread: either another thread's request, or a span this request has no
                // claim on at all. The first is the isolation failure and is reported as one.
                let another_thread = recon
                    .views
                    .iter()
                    .any(|other| other.kind == ViewKind::Span && other.thread.contains(&origin));
                let (assertion, detail) = match another_thread {
                    true => (
                        "request.thread_leak",
                        format!(
                            "shows a block of {}/{}, which belongs to another thread",
                            origin.0, origin.1
                        ),
                    ),
                    false => (
                        "request.provenance",
                        format!(
                            "shows a block of {}/{}, which is neither this request, nor its thread, nor a \
                             tool span it owns",
                            origin.0, origin.1
                        ),
                    ),
                };
                out.push(Violation::new(
                    ViolationView::Request,
                    assertion,
                    &subject,
                    detail,
                ));
                continue;
            }
            // The occurrence itself: the span it claims must actually hold a block read from that carrier at that
            // position. A composition that reported the span but not the occurrence would pass a check over
            // content and span alone.
            if !offered
                .get(&origin)
                .is_some_and(|occurrences| occurrences.contains(&occurrence(block)))
            {
                out.push(Violation::new(
                    ViolationView::Request,
                    "request.provenance",
                    &subject,
                    format!(
                        "claims {}/{} carrier `{}` position `{}`, which that span does not show",
                        origin.0, origin.1, block.carrier, block.position
                    ),
                ));
                continue;
            }
            // And the thread's order: a composed block of an earlier request may not follow one of a later
            // request. The view's own blocks and its tool spans' are not ranked - a tool span's call is placed
            // by the delta that answers it, not by when the span was read.
            if let Some(&at) = rank.get(&origin) {
                if at < last_rank {
                    out.push(Violation::new(
                        ViolationView::Request,
                        "request.provenance",
                        &subject,
                        format!(
                            "shows {}/{} after a request that followed it, so the composition is out of thread \
                             order",
                            origin.0, origin.1
                        ),
                    ));
                }
                last_rank = last_rank.max(at);
            }
        }
    }
}
