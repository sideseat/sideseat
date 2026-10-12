//! A release difference both truths leave open is not a release difference.
//!
//! Results of the calls one response made run concurrently, and each arrives when its tool finishes: their
//! order is a property of the run, not of the conversation, and no truth fixes it (`order::check_sequence`
//! lets them permute). Two releases replaying one scenario may therefore show one batch's results in two
//! orders. That is explained only where **both** fixtures' truths hold the same results as one open batch;
//! anything else - a permutation reaching past the batch, results of two batches swapped, or a batch either
//! truth orders - is still a difference. Each view must still show one order: the delivery variations and the
//! cross-view checks are untouched by this.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::super::predicates::{Shows, shows};
use super::super::recon::{self, Block, Recon, ViewKind};
use super::super::truth::{Fact, Truth};

/// One open batch: the ids of the results in it.
type Batch = BTreeSet<String>;

/// The truth's open batches, each named by its members: runs of two or more consecutive asserted results in
/// one conversation's sequence answering calls of one response, which no edge orders among themselves.
pub(super) fn open_batches(truth: &Truth) -> BTreeSet<Batch> {
    let fact = |id: &str| truth.facts.iter().find(|f| f.id == id);
    let answers: BTreeMap<&str, &str> = truth
        .edges
        .iter()
        .filter(|e| e.kind == "result_of")
        .filter_map(|e| Some((e.from.as_deref()?, e.to.as_deref()?)))
        .collect();
    // The response whose call a result answers.
    let response_of = |id: &str| {
        // A result the truth lists as a response's own part is ordered with that response's parts.
        let result = fact(id)
            .filter(|f| f.kind == "tool_result" && f.require.is_some() && f.call.is_none())?;
        let call = fact(answers.get(result.id.as_str())?)?;
        call.call.as_deref()
    };
    let mut batches = BTreeSet::new();
    for conversation in &truth.conversations {
        let mut run: (Option<&str>, Batch) = (None, Batch::new());
        let mut close = |run: &mut (Option<&str>, Batch)| {
            let members = std::mem::take(&mut run.1);
            if members.len() >= 2 {
                batches.insert(members);
            }
            run.0 = None;
        };
        for id in &conversation.sequence {
            let response = response_of(id);
            if response.is_none() || response != run.0 {
                close(&mut run);
            }
            if let Some(response) = response {
                run.0 = Some(response);
                run.1.insert(id.clone());
            }
        }
        close(&mut run);
    }
    // An order the truth states between two members fixes the batch: an edge between them, or a call listing
    // two of them among its outputs, in the order `order.parts` holds them to.
    batches.retain(|batch| {
        let edge = truth.edges.iter().any(|e| {
            let first = e.before.as_deref().or(e.from.as_deref());
            let second = e.after.as_deref().or(e.to.as_deref());
            first.is_some_and(|f| batch.contains(f)) && second.is_some_and(|s| batch.contains(s))
        });
        let listed = truth.calls.iter().any(|call| {
            call.outputs
                .iter()
                .filter(|id| batch.contains(id.as_str()))
                .count()
                >= 2
        });
        !edge && !listed
    });
    batches
}

/// A fixture's conversation as `invariance::conversation` projects it - every trace view by its stable label,
/// every session view, the feed; each block as role, type and full-content digest - with each run of
/// consecutive blocks showing members of one batch in `open` put in digest order. A block belongs to a batch
/// only when it shows exactly one asserted result, and that result is in it.
pub(super) fn projection(
    recon: &Recon,
    labels: &BTreeMap<String, String>,
    truth: &Truth,
    open: &BTreeSet<Batch>,
) -> Vec<(&'static str, String, Vec<String>)> {
    let batch_of = |id: &str| open.iter().position(|batch| batch.contains(id));
    // Every asserted result competes, open or not: a block two facts could be is in no batch.
    let results: Vec<_> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_result")
        .collect();
    let mut out = Vec::new();
    for view in &recon.views {
        let (tag, key) = match view.kind {
            ViewKind::Trace => (
                "trace",
                labels
                    .get(&view.key)
                    .cloned()
                    .unwrap_or_else(|| view.key.clone()),
            ),
            ViewKind::Session => ("session", view.key.clone()),
            ViewKind::Feed => ("feed", String::new()),
            ViewKind::Span => continue,
        };
        // Each block's result, where it shows exactly one, and that result's batch.
        let members: Vec<Option<&str>> = view
            .blocks
            .iter()
            .map(|block| {
                let shown: Vec<_> = results
                    .iter()
                    .filter(|f| shows(f, block, None) != Shows::No)
                    .collect();
                match shown.as_slice() {
                    [one] if batch_of(&one.id).is_some() => Some(one.id.as_str()),
                    _ => None,
                }
            })
            .collect();
        let batches: Vec<Option<usize>> = members.iter().map(|m| m.and_then(batch_of)).collect();
        let mut digests: Vec<String> = Vec::with_capacity(view.blocks.len());
        let mut start = 0;
        while start < view.blocks.len() {
            let mut end = start + 1;
            if batches[start].is_some() {
                while end < view.blocks.len() && batches[end] == batches[start] {
                    end += 1;
                }
            }
            // One entry per run, tagged with its batch: a block sorted into a run can neither leave it nor be
            // mistaken for an outsider with the same content.
            let mut run: Vec<String> = view.blocks[start..end]
                .iter()
                .zip(&members[start..end])
                .map(|(b, member)| format!("{}={}", member.unwrap_or("-"), b.digest))
                .collect();
            run.sort_unstable();
            let tag = batches[start].map_or_else(|| "-".to_string(), |b| b.to_string());
            digests.push(format!("{tag}:{}", run.join(",")));
            start = end;
        }
        out.push((tag, key, digests));
    }
    out.sort();
    out
}

/// Whether the reconstruction shows each open batch in one order: every span, trace and session view showing two
/// or more of its members orders them consistently - their orders together are acyclic, so one order of the
/// batch agrees with every view - and no view, the feed included, shows one member twice. The feed's order is
/// not held to the others: it orders a batch by when each result arrived, which is its own presentation.
/// Release invariance may set a batch's order aside only where the fixture itself shows one order for it.
pub(super) fn one_order_per_batch(recon: &Recon, truth: &Truth, open: &BTreeSet<Batch>) -> bool {
    let results: Vec<_> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_result")
        .collect();
    for batch in open {
        let mut before: BTreeSet<(&str, &str)> = BTreeSet::new();
        for view in &recon.views {
            let shown: Vec<&str> = view
                .blocks
                .iter()
                .filter_map(|block| {
                    let matched: Vec<_> = results
                        .iter()
                        .filter(|f| shows(f, block, None) != Shows::No)
                        .collect();
                    match matched.as_slice() {
                        [one] if batch.contains(&one.id) => Some(one.id.as_str()),
                        _ => None,
                    }
                })
                .collect();
            if shown.iter().collect::<BTreeSet<_>>().len() != shown.len() {
                return false;
            }
            if view.kind == ViewKind::Feed {
                continue;
            }
            for (at, early) in shown.iter().enumerate() {
                for late in &shown[at + 1..] {
                    before.insert((early, late));
                }
            }
        }
        // Acyclic: repeatedly take a member nothing left precedes; a cycle leaves members none can be taken from.
        let mut left: BTreeSet<&str> = batch.iter().map(String::as_str).collect();
        while let Some(&first) = left
            .iter()
            .find(|m| !before.iter().any(|(a, b)| b == *m && left.contains(a)))
        {
            left.remove(first);
        }
        if !left.is_empty() {
            return false;
        }
    }
    true
}

/// A fixture's reconstruction and the labels its traces carry, built as the goldens build them.
pub(super) fn reconstruction(label: &str, paths: &[PathBuf]) -> (Recon, BTreeMap<String, String>) {
    let (rows, spans) = super::super::read(paths);
    let built = crate::build_golden(label, paths, &rows);
    let labels = built.trace_labels.clone();
    (recon::from_built(label, paths, &built, spans), labels)
}

/// The open batches some block of the reconstruction may show a member of without being that member alone: it
/// shows more than one result, or it holds a member's value under another call id - a view that names the call
/// its own way - so which member it is, and where it sits, cannot be told. Neither the batch's order nor its
/// uniqueness can then be checked, and it is not set aside.
fn ambiguous(recon: &Recon, truth: &Truth, open: &BTreeSet<Batch>) -> BTreeSet<Batch> {
    let results: Vec<_> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_result")
        .collect();
    let mut out = BTreeSet::new();
    for block in recon.views.iter().flat_map(|v| &v.blocks) {
        let by_id: Vec<_> = results
            .iter()
            .filter(|f| shows(f, block, None) != Shows::No)
            .collect();
        // The fact's own matcher on the block as its view would show it had it named the call as the truth does -
        // whatever id it carries, or none.
        let by_value: Vec<_> = results
            .iter()
            .filter(|f| block.is_tool_result() && shows(f, &named_as(block, f), None) != Shows::No)
            .collect();
        let matched: Vec<_> = by_id.iter().chain(&by_value).collect();
        let unique = by_id.len() == 1 && by_value.iter().all(|f| f.id == by_id[0].id);
        if !unique {
            out.extend(
                open.iter()
                    .filter(|batch| matched.iter().any(|f| batch.contains(&f.id)))
                    .cloned(),
            );
        }
    }
    out
}

/// `block` carrying the call id `fact`'s call has in the truth, in its content and beside it.
fn named_as(block: &Block, fact: &Fact) -> Block {
    let mut named = block.clone();
    let id = fact
        .value
        .get("call_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(content) = named.content.as_object_mut() {
        content.insert(
            "tool_use_id".to_string(),
            serde_json::Value::String(id.clone()),
        );
    }
    named.tool_use_id = Some(id);
    named
}

/// Whether the two fixtures' conversations differ only in the order of results both truths leave open.
pub(super) fn explains(
    release: (&Recon, &BTreeMap<String, String>, &Truth),
    current: (&Recon, &BTreeMap<String, String>, &Truth),
) -> bool {
    let mut open: BTreeSet<Batch> = open_batches(release.2)
        .intersection(&open_batches(current.2))
        .cloned()
        .collect();
    for unclear in [
        ambiguous(release.0, release.2, &open),
        ambiguous(current.0, current.2, &open),
    ] {
        open.retain(|batch| !unclear.contains(batch));
    }
    !open.is_empty()
        && one_order_per_batch(release.0, release.2, &open)
        && one_order_per_batch(current.0, current.2, &open)
        && projection(release.0, release.1, release.2, &open)
            == projection(current.0, current.1, current.2, &open)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABEL: &str = "strands/native/tool_use";

    fn fixture() -> (Recon, BTreeMap<String, String>, Truth) {
        let fixtures: BTreeMap<String, Vec<PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        let (recon, labels) = reconstruction(LABEL, &fixtures[LABEL]);
        let truth = super::super::super::Truths::load()
            .of(LABEL)
            .expect("a truth describes the fixture");
        (recon, labels, truth)
    }

    /// Swaps the blocks showing facts `a` and `b` in every view.
    fn swap(recon: &mut Recon, truth: &Truth, a: &str, b: &str) {
        swap_in(recon, truth, a, b, |_| true);
    }

    /// Swaps the blocks showing facts `a` and `b` in the views `kind` selects.
    fn swap_in(recon: &mut Recon, truth: &Truth, a: &str, b: &str, kind: fn(ViewKind) -> bool) {
        let fact = |id: &str| truth.facts.iter().find(|f| f.id == id).expect("the fact");
        let (a, b) = (fact(a), fact(b));
        let mut swapped = 0;
        for view in recon.views.iter_mut().filter(|v| kind(v.kind)) {
            let at = |f| {
                view.blocks
                    .iter()
                    .position(|block| shows(f, block, None) != Shows::No)
            };
            if let (Some(x), Some(y)) = (at(a), at(b)) {
                view.blocks.swap(x, y);
                swapped += 1;
            }
        }
        assert!(swapped > 0, "no view shows both facts");
    }

    /// The four results of the scenario's one parallel response.
    const BATCH: [&str; 4] = ["fact-006", "fact-007", "fact-008", "fact-009"];

    #[test]
    fn the_fixture_has_the_batch_this_test_permutes() {
        let (_, _, truth) = fixture();
        let batch: Batch = BATCH.iter().map(|s| s.to_string()).collect();
        assert_eq!(open_batches(&truth), [batch].into());
    }

    #[test]
    fn results_of_one_open_batch_in_another_order_are_explained() {
        let (recon, labels, truth) = fixture();
        let mut permuted = recon.clone();
        swap(&mut permuted, &truth, BATCH[0], BATCH[2]);
        assert!(explains(
            (&permuted, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }

    #[test]
    fn a_batch_one_view_shows_in_another_order_is_not_explained() {
        let (recon, labels, truth) = fixture();
        for kind in [
            (|k| k == ViewKind::Trace) as fn(ViewKind) -> bool,
            |k| k == ViewKind::Session,
            |k| k == ViewKind::Span,
        ] {
            let mut disagreeing = recon.clone();
            swap_in(&mut disagreeing, &truth, BATCH[0], BATCH[2], kind);
            assert!(!explains(
                (&disagreeing, &labels, &truth),
                (&recon, &labels, &truth)
            ));
        }
        // The feed orders a batch by arrival, and is not held to the others' order.
        let mut feed_only = recon.clone();
        swap_in(&mut feed_only, &truth, BATCH[0], BATCH[2], |k| {
            k == ViewKind::Feed
        });
        assert!(explains(
            (&feed_only, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }

    #[test]
    fn views_whose_orders_together_form_a_cycle_are_not_one_order() {
        let (recon, _, truth) = fixture();
        let open = open_batches(&truth);
        assert!(one_order_per_batch(&recon, &truth, &open));
        let fact = |id: &str| truth.facts.iter().find(|f| f.id == id).expect("the fact");
        let (a, b, c) = (fact(BATCH[0]), fact(BATCH[1]), fact(BATCH[2]));
        // Each non-feed view keeps two members, in pairs that together say a < b < c < a.
        let keep = |view: &mut super::super::super::recon::View, first, second| {
            let blocks = std::mem::take(&mut view.blocks);
            let pick = |f| {
                blocks
                    .iter()
                    .find(|block| shows(f, block, None) != Shows::No)
                    .cloned()
            };
            view.blocks = [pick(first), pick(second)].into_iter().flatten().collect();
        };
        let mut cyclic = recon.clone();
        let mut pairs = [(a, b), (b, c), (c, a)].into_iter().cycle();
        for view in cyclic.views.iter_mut().filter(|v| v.kind != ViewKind::Feed) {
            let (first, second) = pairs.next().expect("cycles");
            keep(view, first, second);
        }
        assert!(!one_order_per_batch(&cyclic, &truth, &open));
    }

    #[test]
    fn a_feed_showing_one_result_twice_is_not_one_order() {
        // Every other view's repeat is a cycle of one; the feed's order is not compared, so it is checked apart.
        let (recon, _, truth) = fixture();
        let open = open_batches(&truth);
        let mut doubled = recon.clone();
        let fact = truth
            .facts
            .iter()
            .find(|f| f.id == BATCH[0])
            .expect("the result");
        let view = doubled
            .views
            .iter_mut()
            .find(|v| v.kind == ViewKind::Feed)
            .expect("the feed");
        let at = view
            .blocks
            .iter()
            .position(|block| shows(fact, block, None) != Shows::No)
            .expect("shown");
        let copy = view.blocks[at].clone();
        view.blocks.insert(at + 1, copy);
        assert!(!one_order_per_batch(&doubled, &truth, &open));
    }

    #[test]
    fn a_result_two_facts_could_be_is_in_no_batch() {
        let (recon, labels, mut truth) = fixture();
        // Another conversation's result with the same call id and value competes for the block.
        let mut twin = truth
            .facts
            .iter()
            .find(|f| f.id == BATCH[0])
            .expect("the result")
            .clone();
        twin.id = "fact-099".to_string();
        truth.facts.push(twin);
        let mut other = truth.conversations[0].clone();
        other.id = "conversation-099".to_string();
        other.sequence = vec!["fact-099".to_string()];
        truth.conversations.push(other);
        // Not even a permutation of two other members - one no view places the ambiguous block between: the
        // batch's order cannot be checked at all.
        let mut permuted = recon.clone();
        swap(&mut permuted, &truth, BATCH[1], BATCH[3]);
        assert!(!explains(
            (&permuted, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }

    #[test]
    fn an_outsider_with_a_member_s_content_cannot_cross_into_the_batch() {
        let (recon, labels, truth) = fixture();
        let fact = |id: &str| truth.facts.iter().find(|f| f.id == id).expect("the fact");
        let trace = recon
            .views
            .iter()
            .find(|v| v.kind == ViewKind::Trace)
            .expect("a trace view");
        let pick = |id: &str| {
            trace
                .blocks
                .iter()
                .find(|block| shows(fact(id), block, None) != Shows::No)
                .cloned()
                .expect("shown")
        };
        // Members whose call ids travel beside their content, and an outsider with the lower one's content and
        // another id: one digest, two different things.
        let beside = |id: &str| {
            let mut block = pick(id);
            let call = block.result_call_id().expect("an id").to_string();
            if let Some(content) = block.content.as_object_mut() {
                content.remove("tool_use_id");
            }
            block.tool_use_id = Some(call);
            block.refresh();
            assert!(shows(fact(id), &block, None) != Shows::No);
            block
        };
        let (mut low, mut high) = (beside(BATCH[0]), beside(BATCH[1]));
        if high.digest < low.digest {
            std::mem::swap(&mut low, &mut high);
        }
        let mut outsider = low.clone();
        outsider.tool_use_id = Some("call-outside".to_string());
        outsider.refresh();
        assert_eq!(low.digest, outsider.digest);
        let only_trace = |blocks: Vec<super::super::super::recon::Block>| {
            let mut recon = recon.clone();
            recon.views.retain(|v| v.kind == ViewKind::Trace);
            recon.views.truncate(1);
            recon.views[0].blocks = blocks;
            recon
        };
        // Flattened, both read low, low, high; only the runs say the outsider moved into the batch.
        let release = only_trace(vec![low.clone(), outsider.clone(), high.clone()]);
        let current = only_trace(vec![outsider, high, low]);
        assert!(!explains(
            (&release, &labels, &truth),
            (&current, &labels, &truth)
        ));
    }

    #[test]
    fn a_result_a_view_names_its_own_way_closes_its_batch() {
        let (recon, labels, semantic) = fixture();
        // The same result asserted by the error-message matcher: a part of its text, not its whole value.
        let mut by_message = semantic.clone();
        let member = by_message
            .facts
            .iter_mut()
            .find(|f| f.id == BATCH[0])
            .expect("the result");
        member.require.as_mut().expect("asserted").matcher = "error_message".to_string();
        member.value["value"] = serde_json::json!("Paris");
        member.value["is_error"] = serde_json::json!(true);
        for truth in [&semantic, &by_message] {
            let fact = |id: &str| truth.facts.iter().find(|f| f.id == id).expect("the fact");
            let mut renamed = recon.clone();
            // One span view names the first result's call its own way: it still holds that result's value.
            let span = renamed
                .views
                .iter_mut()
                .filter(|v| v.kind == ViewKind::Span)
                .find(|v| {
                    v.blocks
                        .iter()
                        .any(|b| shows(fact(BATCH[0]), b, None) != Shows::No)
                })
                .expect("a span view showing the batch");
            let block = span
                .blocks
                .iter_mut()
                .find(|b| shows(fact(BATCH[0]), b, None) != Shows::No)
                .expect("shown");
            block.content["tool_use_id"] = serde_json::json!("renamed-call");
            block.tool_use_id = Some("renamed-call".to_string());
            block.refresh();
            // The trace and session views put that result after the second; the span cannot be checked
            // against them.
            swap_in(&mut renamed, truth, BATCH[0], BATCH[1], |k| {
                matches!(k, ViewKind::Trace | ViewKind::Session)
            });
            let open = open_batches(truth);
            assert_eq!(
                ambiguous(&renamed, truth, &open),
                open,
                "{}",
                fact(BATCH[0]).id
            );
            assert!(!explains(
                (&renamed, &labels, truth),
                (&recon, &labels, truth)
            ));
        }
    }

    #[test]
    fn a_provider_run_that_found_nothing_and_lost_its_id_closes_its_batch() {
        let (mut recon, labels, mut truth) = fixture();
        let fact = |truth: &Truth, id: &str| {
            truth
                .facts
                .iter()
                .find(|f| f.id == id)
                .cloned()
                .expect("the fact")
        };
        let original = fact(&truth, BATCH[0]);
        // The first result becomes a provider's run that found nothing, asserted by `contains`.
        let member = truth
            .facts
            .iter_mut()
            .find(|f| f.id == BATCH[0])
            .expect("the result");
        member.require.as_mut().expect("asserted").matcher = "contains".to_string();
        member.value["value"] = serde_json::json!({"sources": []});
        for view in &mut recon.views {
            for block in view
                .blocks
                .iter_mut()
                .filter(|b| shows(&original, b, None) != Shows::No)
            {
                block.content["content"] = serde_json::json!({"sources": []});
                block.content["provider_executed"] = serde_json::json!(true);
                block.refresh();
            }
        }
        let found_nothing = fact(&truth, BATCH[0]);
        let mut lost = recon.clone();
        // One span view shows that run with no call id at all.
        let span = lost
            .views
            .iter_mut()
            .filter(|v| v.kind == ViewKind::Span)
            .find(|v| {
                v.blocks
                    .iter()
                    .any(|b| shows(&found_nothing, b, None) != Shows::No)
            })
            .expect("a span view showing it");
        let block = span
            .blocks
            .iter_mut()
            .find(|b| shows(&found_nothing, b, None) != Shows::No)
            .expect("shown");
        if let Some(content) = block.content.as_object_mut() {
            content.remove("tool_use_id");
        }
        block.tool_use_id = None;
        block.refresh();
        swap_in(&mut lost, &truth, BATCH[0], BATCH[1], |k| {
            matches!(k, ViewKind::Trace | ViewKind::Session)
        });
        let open = open_batches(&truth);
        assert_eq!(ambiguous(&lost, &truth, &open), open);
        assert!(!explains(
            (&lost, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }

    #[test]
    fn members_with_one_content_cannot_trade_places_across_a_block() {
        let (recon, labels, mut truth) = fixture();
        let fact = |truth: &Truth, id: &str| {
            truth
                .facts
                .iter()
                .find(|f| f.id == id)
                .cloned()
                .expect("the fact")
        };
        let trace = recon
            .views
            .iter()
            .find(|v| v.kind == ViewKind::Trace)
            .expect("a trace view");
        let pick = |truth: &Truth, id: &str| {
            trace
                .blocks
                .iter()
                .find(|block| shows(&fact(truth, id), block, None) != Shows::No)
                .cloned()
                .expect("shown")
        };
        // Two members with one value and their call ids beside it: one digest, two results.
        let (a, b, c) = (
            pick(&truth, BATCH[0]),
            pick(&truth, BATCH[1]),
            pick(&truth, BATCH[2]),
        );
        let value = fact(&truth, BATCH[0]).value["value"].clone();
        truth
            .facts
            .iter_mut()
            .find(|f| f.id == BATCH[1])
            .expect("B")
            .value["value"] = value;
        let beside = |mut block: super::super::super::recon::Block, call: String| {
            if let Some(content) = block.content.as_object_mut() {
                content.remove("tool_use_id");
            }
            block.tool_use_id = Some(call);
            block.refresh();
            block
        };
        let b_call = b.result_call_id().expect("an id").to_string();
        let a = beside(a.clone(), a.result_call_id().expect("an id").to_string());
        let b = beside(a.clone(), b_call);
        assert_eq!(a.digest, b.digest);
        let outsider = trace
            .blocks
            .iter()
            .find(|block| block.role == "assistant")
            .cloned()
            .expect("an assistant block");
        let only_trace = |blocks: Vec<super::super::super::recon::Block>| {
            let mut recon = recon.clone();
            recon.views.retain(|v| v.kind == ViewKind::Trace);
            recon.views.truncate(1);
            recon.views[0].blocks = blocks;
            recon
        };
        let release = only_trace(vec![a.clone(), outsider.clone(), b.clone(), c.clone()]);
        let current = only_trace(vec![b, outsider, c, a]);
        // Two results with one value are ambiguous, so the batch closes; the runs keep their members apart even
        // where it would not.
        let open = open_batches(&truth);
        assert_ne!(
            projection(&release, &labels, &truth, &open),
            projection(&current, &labels, &truth, &open)
        );
        assert!(!explains(
            (&release, &labels, &truth),
            (&current, &labels, &truth)
        ));
    }

    #[test]
    fn a_result_moved_past_its_batch_is_not_explained() {
        let (recon, labels, truth) = fixture();
        let mut moved = recon.clone();
        // The final answer follows the batch in every view; exchanging it with a result reaches past the batch.
        swap(&mut moved, &truth, BATCH[3], "fact-010");
        assert!(!explains(
            (&moved, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }

    #[test]
    fn results_of_two_batches_exchanged_are_not_explained() {
        let (recon, labels, mut truth) = fixture();
        // A fact between the second and third result splits the run into two batches of two.
        let sequence = &mut truth.conversations[0].sequence;
        let at = sequence
            .iter()
            .position(|id| id == BATCH[2])
            .expect("in sequence");
        sequence.insert(at, "fact-001".to_string());
        assert_eq!(open_batches(&truth).len(), 2);
        let mut exchanged = recon.clone();
        swap(&mut exchanged, &truth, BATCH[1], BATCH[2]);
        assert!(!explains(
            (&exchanged, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }

    #[test]
    fn results_answering_two_responses_are_two_batches() {
        let (recon, labels, mut truth) = fixture();
        // The calls the third and fourth results answer, made by another response.
        for call in ["fact-004", "fact-005"] {
            let fact = truth
                .facts
                .iter_mut()
                .find(|f| f.id == call)
                .expect("the call");
            fact.call = Some("call-009".to_string());
        }
        assert_eq!(open_batches(&truth).len(), 2);
        let mut exchanged = recon.clone();
        swap(&mut exchanged, &truth, BATCH[1], BATCH[2]);
        assert!(!explains(
            (&exchanged, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }

    #[test]
    fn a_batch_either_truth_orders_is_not_explained() {
        let (recon, labels, truth) = fixture();
        let mut permuted = recon.clone();
        swap(&mut permuted, &truth, BATCH[0], BATCH[2]);
        // A response's own parts are ordered with it: two of the results made parts of call-002, and two of
        // them listed among its outputs - each alone states their order.
        let mut as_parts = truth.clone();
        for id in [BATCH[0], BATCH[2]] {
            as_parts
                .facts
                .iter_mut()
                .find(|f| f.id == id)
                .expect("the result")
                .call = Some("call-002".to_string());
        }
        let mut as_outputs = truth.clone();
        let call = as_outputs
            .calls
            .iter_mut()
            .find(|c| c.id == "call-002")
            .expect("call-002");
        call.outputs
            .extend([BATCH[0].to_string(), BATCH[2].to_string()]);
        for ordered in [&as_parts, &as_outputs] {
            assert!(
                !open_batches(ordered)
                    .iter()
                    .any(|b| b.contains(BATCH[0]) && b.contains(BATCH[2]))
            );
            assert!(!explains(
                (&permuted, &labels, ordered),
                (&recon, &labels, &truth)
            ));
            assert!(!explains(
                (&permuted, &labels, &truth),
                (&recon, &labels, ordered)
            ));
        }
    }

    #[test]
    fn a_changed_result_is_not_explained() {
        let (recon, labels, truth) = fixture();
        let mut changed = recon.clone();
        swap(&mut changed, &truth, BATCH[0], BATCH[2]);
        for view in changed
            .views
            .iter_mut()
            .filter(|v| v.kind != ViewKind::Span)
        {
            if let Some(block) = view.blocks.iter_mut().find(|b| b.is_tool_result()) {
                block.content["content"] = serde_json::json!("something else");
                block.refresh();
            }
        }
        assert!(!explains(
            (&changed, &labels, &truth),
            (&recon, &labels, &truth)
        ));
    }
}
