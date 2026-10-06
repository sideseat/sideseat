//! Mutations of what a span shows its call was sent (rubric v3, slice 1).

use super::recon::{Recon, ViewKind};
use super::truth::{Occurrence, RequestMessage, Truth};

/// The span view of the first call whose request was recorded, with the indices of its input blocks.
fn recorded_inputs(truth: &Truth, recon: &Recon) -> Option<(usize, Vec<usize>)> {
    let recorded = truth.requests.get(&recon.fixture)?;
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    recorded.calls.keys().find_map(|call| {
        let span = &recon.generations[*matching.span_of.get(call)?].span;
        let view = recon
            .views
            .iter()
            .position(|v| v.kind == ViewKind::Span && &v.key == span)?;
        let inputs: Vec<usize> = recon.views[view]
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.output)
            .map(|(i, _)| i)
            .collect();
        (!inputs.is_empty()).then_some((view, inputs))
    })
}

/// A message the call was sent, gone from its span's input.
pub(super) fn drop_sent_message(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((view, inputs)) = recorded_inputs(truth, recon) else {
        return false;
    };
    recon.views[view].blocks.remove(inputs[0]);
    true
}

/// A message the call was sent, shown twice on its span's input.
pub(super) fn repeat_sent_message(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((view, inputs)) = recorded_inputs(truth, recon) else {
        return false;
    };
    let copy = recon.views[view].blocks[inputs[0]].clone();
    recon.views[view].blocks.insert(inputs[0] + 1, copy);
    true
}

/// Two different messages the call was sent, shown in each other's place.
pub(super) fn swap_sent_messages(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((view, inputs)) = recorded_inputs(truth, recon) else {
        return false;
    };
    let blocks = &mut recon.views[view].blocks;
    let Some(pair) = inputs
        .windows(2)
        .find(|w| blocks[w[0]].digest != blocks[w[1]].digest)
    else {
        return false;
    };
    blocks.swap(pair[0], pair[1]);
    true
}

/// An instruction the call was never sent, shown on its span's input.
pub(super) fn invent_sent_message(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((view, inputs)) = recorded_inputs(truth, recon) else {
        return false;
    };
    let mut invented = recon.views[view].blocks[inputs[0]].clone();
    invented.role = "user".into();
    invented.kind = "text".into();
    invented.content = serde_json::json!({"type": "text", "text": "An instruction nobody sent."});
    invented.refresh();
    recon.views[view].blocks.insert(inputs[0], invented);
    true
}

/// A user message the call was sent, shown under the assistant's role.
pub(super) fn reassign_sent_role(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((view, inputs)) = recorded_inputs(truth, recon) else {
        return false;
    };
    let blocks = &mut recon.views[view].blocks;
    let Some(&index) = inputs
        .iter()
        .find(|&&i| blocks[i].role == "user" && blocks[i].kind == "text")
    else {
        return false;
    };
    blocks[index].role = "assistant".into();
    blocks[index].refresh();
    true
}

/// Add one part to what a call was sent, as a user turn of its own message.
fn claim_sent_text(truth: &mut Truth, recon: &Recon, text: &str) -> bool {
    let Some(call) = first_recorded_call(truth, recon) else {
        return false;
    };
    let Some(recorded) = truth.requests.get_mut(&recon.fixture) else {
        return false;
    };
    let Some(request) = recorded.calls.get_mut(&call) else {
        return false;
    };
    request.messages.push(RequestMessage {
        role: "user".to_string(),
        parts: vec![Occurrence {
            part: serde_json::json!({"type": "text", "text": text}),
            new_fact: None,
            replay_of: None,
            new: Some("rq-claimed".to_string()),
            lineage_unknown: None,
        }],
    });
    true
}

/// The first call whose request the fixture recorded and whose span its output established.
fn first_recorded_call(truth: &Truth, recon: &Recon) -> Option<String> {
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    let recorded = truth.requests.get(&recon.fixture)?;
    recorded
        .calls
        .keys()
        .find(|call| {
            matching.span_of.contains_key(*call)
                && truth.calls.iter().any(|c| {
                    &&c.id == call && c.succeeded() && !matching.unmatchable.contains(&c.id)
                })
        })
        .cloned()
}

/// A part the call was sent that no payload carries: the producer did not export it, which the rubric
/// must report as that framework's limitation rather than as a reconstruction defect.
pub(super) fn claim_an_unexported_part(truth: &mut Truth, recon: &mut Recon) -> bool {
    claim_sent_text(
        truth,
        recon,
        "A system preamble this framework's telemetry never carried, in any payload.",
    )
}

/// A part the call was sent that the payloads do carry, which no input shows: the reconstruction lost it.
pub(super) fn claim_a_lost_part(truth: &mut Truth, recon: &mut Recon) -> bool {
    let answer = truth
        .facts
        .iter()
        .find(|f| f.kind == "text" && f.call.is_some() && f.text().chars().count() > 40)
        .map(|f| f.text().to_string());
    let Some(text) = answer else {
        return false;
    };
    claim_sent_text(truth, recon, &text)
}
