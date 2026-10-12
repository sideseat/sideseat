//! The relative order of one response's parallel tool calls (`call_order_not_exported`): no payload holds two
//! of them together, so no telemetry states which came first. Only the order among them is withdrawn - each
//! call's presence, completeness, its place before its own result and against the rest of the conversation
//! stay asserted.

use super::haystack::Haystack;
use super::{MAX_DEPTH, MIN_ID, Proof, find_node, find_string, wire_id};
use crate::message_truth::predicates::json_eq;
use crate::message_truth::truth::{Fact, Truth};

/// The response's tool calls, as truth facts.
pub(super) fn parallel_calls<'t>(truth: &'t Truth, call_id: &str) -> Vec<&'t Fact> {
    let Some(call) = truth.calls.iter().find(|c| c.id == call_id) else {
        return Vec::new();
    };
    call.outputs
        .iter()
        .filter_map(|id| truth.facts.iter().find(|f| &f.id == id))
        .filter(|f| f.kind == "tool_call")
        .collect()
}

/// `call_order_not_exported` holds where the response has at least two tool calls, each with a wire id, and
/// no carrier of the capture - a span with its events, or a log record: coarser than one attribute, so the
/// test is the stricter - holds two of them. A carrier holds a call where it holds the call's id, or a value
/// equal to its arguments: a request re-sending the assistant turn would hold both, with or without ids.
pub(super) fn prove_call_order(truth: &Truth, call_id: &str, haystack: &Haystack) -> Proof {
    let calls = parallel_calls(truth, call_id);
    if calls.len() < 2 {
        return Proof::Unprovable(format!("{call_id} has fewer than two tool calls"));
    }
    if let Some(at) = haystack.undecoded.first() {
        return Proof::Unprovable(format!(
            "{at} is encoded deeper than the search decodes ({MAX_DEPTH} layers)"
        ));
    }
    let mut ids = Vec::with_capacity(calls.len());
    for fact in &calls {
        let Some(id) = wire_id(fact).filter(|id| id.len() >= MIN_ID) else {
            return Proof::Unprovable(format!("{} has no wire id to search for", fact.id));
        };
        ids.push(id);
    }
    for carrier in &haystack.carriers {
        let held: Vec<&str> = calls
            .iter()
            .zip(&ids)
            .filter(|(fact, id)| {
                find_string(carrier, id).is_some()
                    || find_node(carrier, |node| json_eq(node, &fact.value["arguments"])).is_some()
            })
            .map(|(fact, _)| fact.id.as_str())
            .collect();
        if held.len() >= 2 {
            return Proof::Present(format!(
                "one carrier{} holds {}",
                carrier
                    .span
                    .as_deref()
                    .map(|s| format!(" of span {s}"))
                    .unwrap_or_default(),
                held.join(" and ")
            ));
        }
    }
    Proof::Absent
}

#[cfg(test)]
mod tests {
    use super::super::Proof;
    use super::super::haystack::{Carrier, Haystack};
    use super::{parallel_calls, prove_call_order};
    use crate::message_truth::mutations::baseline;
    use crate::message_truth::recon::ViewKind;

    const FIXTURES: [&str; 3] = [
        "ag2/native@1.0.2/tool_use",
        "ag2/native@1.0.3/tool_use",
        "ag2/native@1.0.4/tool_use",
    ];

    /// The response whose call order each fixture declares unexported.
    fn gapped(truth: &crate::message_truth::truth::Truth, fixture: &str) -> String {
        truth
            .gaps
            .iter()
            .find(|g| g.reason == "call_order_not_exported" && g.holds_for(fixture))
            .and_then(|g| g.subject.clone())
            .expect("the fixture declares a call order unexported")
    }

    fn haystack(fixture: &str) -> Haystack {
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        Haystack::of_fixture(&fixtures[fixture])
    }

    /// Proven on every capture as recorded; refused as soon as one carrier holds two of the calls - by their
    /// ids, or by their arguments alone, as a request re-sending the assistant turn would.
    #[test]
    fn a_call_order_is_unexported_only_while_no_payload_holds_two_of_its_calls() {
        for fixture in FIXTURES {
            let (truth, _, _) = baseline(fixture);
            let call = gapped(&truth, fixture);
            let mut found = haystack(fixture);
            assert_eq!(
                prove_call_order(&truth, &call, &found),
                Proof::Absent,
                "{fixture}: proven as captured"
            );
            let calls = parallel_calls(&truth, &call);
            assert!(
                calls.len() >= 2,
                "{fixture}: a response with parallel calls"
            );

            // One payload naming two of the calls by id.
            let ids: Vec<String> = calls
                .iter()
                .map(|f| super::wire_id(f).expect("a wire id").to_string())
                .collect();
            found.carriers.push(Carrier {
                span: Some("feedfacefeedface".to_string()),
                strings: vec![(
                    "span.attributes.calls".to_string(),
                    format!("{} {}", ids[0], ids[1]),
                )],
                ..Carrier::default()
            });
            assert!(
                matches!(prove_call_order(&truth, &call, &found), Proof::Present(_)),
                "{fixture}: a payload holding two call ids must refuse the proof"
            );

            // A later request re-sending the assistant turn: both calls' arguments, no ids.
            let mut resent = haystack(fixture);
            resent.carriers.push(Carrier {
                span: Some("feedfacefeedface".to_string()),
                nodes: calls
                    .iter()
                    .take(2)
                    .enumerate()
                    .map(|(i, f)| {
                        (
                            format!("gen_ai.input.messages[1].tool_calls[{i}].arguments"),
                            f.value["arguments"].clone(),
                        )
                    })
                    .collect(),
                ..Carrier::default()
            });
            assert!(
                matches!(prove_call_order(&truth, &call, &resent), Proof::Present(_)),
                "{fixture}: a request re-sending the assistant turn must refuse the proof"
            );
        }
    }

    /// With their order withdrawn, the views must still agree: the response's calls appear in one identical
    /// order in every trace and session view, and in the same order on every reconstruction.
    #[test]
    fn parallel_calls_whose_order_is_unexported_read_alike_in_every_view() {
        for fixture in FIXTURES {
            let (truth, recon, _) = baseline(fixture);
            let call = gapped(&truth, fixture);
            let ids: Vec<String> = parallel_calls(&truth, &call)
                .iter()
                .map(|f| super::wire_id(f).expect("a wire id").to_string())
                .collect();
            let order_in = |recon: &crate::message_truth::recon::Recon| -> Vec<Vec<String>> {
                recon
                    .views
                    .iter()
                    .filter(|v| matches!(v.kind, ViewKind::Trace | ViewKind::Session))
                    .map(|v| {
                        v.blocks
                            .iter()
                            .filter(|b| b.kind == "tool_use")
                            .filter_map(|b| b.content["id"].as_str())
                            .filter(|id| ids.iter().any(|i| i == id))
                            .map(str::to_string)
                            .collect()
                    })
                    .collect()
            };
            let orders = order_in(&recon);
            assert!(!orders.is_empty(), "{fixture}: trace and session views");
            for order in &orders {
                assert_eq!(order.len(), ids.len(), "{fixture}: every call shown once");
                assert_eq!(order, &orders[0], "{fixture}: one order in every view");
            }
            let (_, again, _) = baseline(fixture);
            assert_eq!(
                order_in(&again),
                orders,
                "{fixture}: the same order every time"
            );
        }
    }

    /// Moves the first block of `moved` in front of the first block of `before` in every view of `kind`.
    fn move_before(
        recon: &mut crate::message_truth::recon::Recon,
        kind: ViewKind,
        moved: &crate::message_truth::truth::Fact,
        before: &crate::message_truth::truth::Fact,
    ) {
        use crate::message_truth::predicates::{Shows, shows};
        for view in recon.views.iter_mut().filter(|v| v.kind == kind) {
            let first = |blocks: &[crate::message_truth::recon::Block], f| {
                blocks.iter().position(|b| shows(f, b, None) != Shows::No)
            };
            if let (Some(a), Some(c)) = (first(&view.blocks, moved), first(&view.blocks, before)) {
                let block = view.blocks.remove(a);
                let at = if a < c { c - 1 } else { c };
                view.blocks.insert(at, block);
            }
        }
    }

    /// The gap frees the parallel calls from an order among themselves and from nothing else: swapping two
    /// of them adds nothing, while dropping one, showing a result before its call, or a call before the
    /// prompt that asked for it is still caught.
    #[test]
    fn only_the_order_among_the_parallel_calls_is_withdrawn() {
        use crate::message_truth::mutate::{locate, remove_positions};
        use crate::message_truth::mutations::added;
        let fixture = "ag2/native@1.0.4/tool_use";
        let (truth, recon, baseline) = baseline(fixture);
        let call = gapped(&truth, fixture);
        let calls = parallel_calls(&truth, &call);
        let fact = |id: &str| truth.facts.iter().find(|f| f.id == id).expect("a fact");

        let mut swapped = recon.clone();
        move_before(&mut swapped, ViewKind::Trace, calls[1], calls[0]);
        move_before(&mut swapped, ViewKind::Session, calls[1], calls[0]);
        let new = added(&truth, &swapped, &baseline);
        assert!(
            new.iter().all(|v| !v.starts_with("order.")),
            "two parallel calls swapped is no order defect: {new:?}"
        );

        let mut dropped = recon.clone();
        let positions: Vec<(usize, usize)> = locate(calls[0], &dropped)
            .into_iter()
            .filter(|&(v, _)| dropped.views[v].kind == ViewKind::Trace)
            .collect();
        assert!(!positions.is_empty(), "the trace view shows the call");
        remove_positions(&mut dropped, positions);
        let new = added(&truth, &dropped, &baseline);
        assert!(
            new.iter()
                .any(|v| v.contains(&format!(":{}", calls[0].id)) && v.contains("missing")),
            "a dropped parallel call is still caught: {new:?}"
        );

        let result = truth
            .edges
            .iter()
            .find(|e| e.kind == "result_of" && e.to.as_deref() == Some(calls[0].id.as_str()))
            .and_then(|e| e.from.as_deref())
            .map(fact)
            .expect("the call's result");
        let mut early = recon.clone();
        move_before(&mut early, ViewKind::Trace, result, calls[0]);
        let new = added(&truth, &early, &baseline);
        assert!(
            new.iter().any(|v| v.starts_with("order.")),
            "a result before its own call is still caught: {new:?}"
        );

        let prompt = truth
            .facts
            .iter()
            .find(|f| f.kind == "user_text")
            .expect("the prompt");
        let mut before_prompt = recon.clone();
        move_before(&mut before_prompt, ViewKind::Trace, calls[0], prompt);
        let new = added(&truth, &before_prompt, &baseline);
        assert!(
            new.iter().any(|v| v.starts_with("order.")),
            "a parallel call before the prompt that asked for it is still caught: {new:?}"
        );
    }
}
