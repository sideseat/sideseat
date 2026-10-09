//! Order: the truth's edges, and its conversation sequence, in every view that holds both ends.

use std::collections::{BTreeMap, BTreeSet};

use super::checks::{Assigned, Context, Scope};
use super::recon::ViewKind;
use super::{Violation, ViolationView};

/// `before` precedes `after` in every view that holds both: (assertion, subject, before, after).
type Constraint = (&'static str, String, Vec<String>, Vec<String>);

fn constraints(context: &Context<'_>, feed: bool) -> Vec<Constraint> {
    let truth = context.truth;
    let outputs = |call: &str| -> Vec<String> {
        truth
            .calls
            .iter()
            .find(|c| c.id == call)
            .map(|c| c.outputs.clone())
            .unwrap_or_default()
    };
    let mut out: Vec<Constraint> = Vec::new();
    // Part order within one response holds in every view, the feed included - for an unexported
    // response too, since its calls are executed, and recorded, in the order it lists them. Each of
    // those is recorded on its own, so the feed, newest first, lists them in reverse.
    for call in &truth.calls {
        let reversed = feed && context.unexported(&call.id);
        for pair in call.outputs.windows(2) {
            let (first, second) = if reversed {
                (&pair[1], &pair[0])
            } else {
                (&pair[0], &pair[1])
            };
            out.push((
                "order.parts",
                format!("{}<{}", pair[0], pair[1]),
                vec![first.clone()],
                vec![second.clone()],
            ));
        }
    }
    let inputs_of = |call: &str| -> Vec<String> {
        let mut inputs = Vec::new();
        for edge in &truth.edges {
            if edge.kind == "prompt_of" && edge.to.as_deref() == Some(call) {
                inputs.extend(edge.from.clone());
            }
        }
        inputs
    };
    let results_of = |call: &str| -> Vec<String> {
        truth
            .edges
            .iter()
            .filter(|e| e.kind == "result_of")
            .filter(|e| {
                e.to.as_deref()
                    .and_then(|t| context.fact(t))
                    .and_then(|t| t.call.as_deref())
                    == Some(call)
            })
            .filter_map(|e| e.from.clone())
            .collect()
    };
    for edge in &truth.edges {
        match (edge.kind.as_str(), feed) {
            ("call_order", _) => {
                let (Some(a), Some(b)) = (edge.before.as_deref(), edge.after.as_deref()) else {
                    continue;
                };
                // The feed is newest response first, so it states the calls in reverse - except two calls
                // recorded on one span (`call_span_not_exported`), which it lists as that span does, oldest
                // first.
                let one_span = context
                    .matching
                    .span_showing(a)
                    .is_some_and(|span| context.matching.span_showing(b) == Some(span));
                let (first, second) = if feed && !one_span { (b, a) } else { (a, b) };
                out.push((
                    "order.calls",
                    format!("{a}<{b}"),
                    outputs(first),
                    outputs(second),
                ));
                if !feed {
                    let mut between = inputs_of(b);
                    between.extend(results_of(a));
                    // An unexported response's calls are known only one by one, each beside its result,
                    // so nothing places all of them before the results.
                    if !context.unexported(a) {
                        out.push((
                            "order.inputs_after_previous",
                            format!("{a}<inputs({b})"),
                            outputs(a),
                            between,
                        ));
                    }
                    out.push((
                        "order.inputs_before_next",
                        format!("inputs({b})<{b}"),
                        results_of(a),
                        outputs(b),
                    ));
                }
            }
            ("prompt_of", false) => {
                let (Some(prompt), Some(call)) = (edge.from.as_deref(), edge.to.as_deref()) else {
                    continue;
                };
                out.push((
                    "order.prompt",
                    format!("{prompt}<{call}"),
                    vec![prompt.to_string()],
                    outputs(call),
                ));
            }
            ("result_of", false) => {
                let (Some(result), Some(call)) = (edge.from.as_deref(), edge.to.as_deref()) else {
                    continue;
                };
                out.push((
                    "order.result",
                    format!("{call}<{result}"),
                    vec![call.to_string()],
                    vec![result.to_string()],
                ));
            }
            _ => {}
        }
    }
    // A request's instruction frames the turns it was sent with: in a trace or session view the instruction
    // comes before the prompt it framed, whichever span's copy of that prompt the view kept. Not in the
    // feed, which lists newest first and states no order among one request's inputs.
    if let Some(recorded) = truth.requests.get(&context.recon.fixture).filter(|_| !feed) {
        for (call, request) in &recorded.calls {
            let instructions: Vec<String> = request
                .system
                .iter()
                .filter_map(|o| o.new_fact.as_ref().or(o.replay_of.as_ref()))
                .filter(|id| context.fact(id).is_some())
                .cloned()
                .collect();
            // The prompts this request itself carried: a framework that hands an agent its own rendering
            // of the question sent the rendering, and the question it renders was told before the
            // agent's instruction existed.
            let carried: BTreeSet<&str> = request
                .messages
                .iter()
                .flat_map(|m| m.parts.iter())
                .filter_map(|o| o.new_fact.as_deref())
                .collect();
            // Its prompts and the attachments sent with them.
            let turns: Vec<String> = carried
                .iter()
                .filter(|id| {
                    context.fact(id).is_some_and(|f| match f.kind.as_str() {
                        "user_text" => inputs_of(call).iter().any(|p| p == *id),
                        "user_media" => true,
                        _ => false,
                    })
                })
                .map(|id| id.to_string())
                .collect();
            if !instructions.is_empty() && !turns.is_empty() {
                out.push((
                    "order.frame",
                    format!("{call}:instruction<prompt"),
                    instructions,
                    turns,
                ));
            }
        }
    }
    if truth.topology == "sessions" && !feed {
        for pair in truth.conversations.windows(2) {
            out.push((
                "order.conversations",
                format!("{}<{}", pair[0].id, pair[1].id),
                pair[0].sequence.clone(),
                pair[1].sequence.clone(),
            ));
        }
    }
    out
}

pub(super) fn check(
    context: &Context<'_>,
    scope: &Scope<'_>,
    assigned: &Assigned,
    out: &mut Vec<Violation>,
) {
    let at = |id: &str| -> Option<(usize, usize)> {
        assigned
            .block_of
            .get(id)
            .map(|&k| (scope.blocks[k].0, scope.blocks[k].1))
    };
    let mut seen = BTreeSet::new();
    // A split-off trace's portion of the feed is one span's record - what its call was sent, then its answer -
    // listed oldest first, as the span lists it.
    let feed = scope.kind == ViewKind::Feed && scope.split.is_none();
    for (assertion, subject, before, after) in constraints(context, feed) {
        let violated = before.iter().filter_map(|b| at(b)).any(|(view, early)| {
            after
                .iter()
                .filter_map(|a| at(a))
                .any(|(other, late)| other == view && late <= early)
        });
        if violated && seen.insert((assertion, subject.clone())) {
            out.push(Violation::new(
                ViolationView::from(scope.kind),
                assertion,
                &subject,
                "out of order".to_string(),
            ));
        }
    }
    if !feed && scope.kind != ViewKind::Span {
        check_sequence(context, scope, assigned, out);
    }
}

/// A trace or session view states the conversation in the truth's order, not merely an order the
/// edges allow.
///
/// Every view holds its facts in the order of their conversations' sequences - conversations in
/// order, then position. The one freedom is within a run of inputs between two responses: results
/// of parallel calls arrive in completion order, and a prompt's text and attachments in any.
fn check_sequence(
    context: &Context<'_>,
    scope: &Scope<'_>,
    assigned: &Assigned,
    out: &mut Vec<Violation>,
) {
    // (conversation, group): a response's parts are each a group; consecutive inputs share one.
    let mut rank: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (c, conversation) in context.truth.conversations.iter().enumerate() {
        let mut group = 0usize;
        let mut in_inputs = false;
        for id in &conversation.sequence {
            // An unexported response's parts are not a response anyone recorded: with their results they
            // form one run of inputs, which may permute.
            let output = context
                .fact(id)
                .and_then(|f| f.call.as_deref())
                .is_some_and(|c| !context.unexported(c));
            if output || !in_inputs {
                group += 1;
            }
            in_inputs = !output;
            rank.insert(id.as_str(), (c, group));
        }
    }
    let mut by_view: BTreeMap<usize, Vec<(usize, &str)>> = BTreeMap::new();
    for (fact, &at) in &assigned.block_of {
        let (view, index, _) = scope.blocks[at];
        by_view
            .entry(view)
            .or_default()
            .push((index, fact.as_str()));
    }
    let mut reported = BTreeSet::new();
    for facts in by_view.values_mut() {
        facts.sort();
        for pair in facts.windows(2) {
            let (early, late) = (pair[0].1, pair[1].1);
            if rank.get(early) > rank.get(late) && reported.insert(late) {
                out.push(Violation::new(
                    ViolationView::from(scope.kind),
                    "order.sequence",
                    &format!("{late} before {early}"),
                    "shown before a fact the truth sequences ahead of it".to_string(),
                ));
            }
        }
    }
}
