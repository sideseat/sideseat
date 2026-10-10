//! Mutations of what a span shows its call was sent (rubric v3, slice 1).

use serde_json::Value;

use super::mutate::swap_in_views;
use super::recon::{Block, Recon, ViewKind};
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

/// Two different parts the call was sent, shown in each other's place.
pub(super) fn swap_sent_messages(truth: &mut Truth, recon: &mut Recon) -> bool {
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    let Some((view, shown)) = super::requests::sequenced_inputs(truth, recon, &matching) else {
        return false;
    };
    let blocks = &mut recon.views[view].blocks;
    let Some(pair) = shown
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
            renders: Vec::new(),
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

/// Swaps a request's instruction with the prompt it framed, in every view that holds both.
pub(super) fn swap_instruction_and_prompt(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(recorded) = truth.requests.get(&recon.fixture) else {
        return false;
    };
    for (call, request) in &recorded.calls {
        let instruction = request
            .system
            .iter()
            .filter_map(|o| o.new_fact.as_ref().or(o.replay_of.as_ref()))
            .find_map(|id| truth.facts.iter().find(|f| &f.id == id));
        let prompt = truth
            .edges
            .iter()
            .filter(|e| e.kind == "prompt_of" && e.to.as_deref() == Some(call.as_str()))
            .filter_map(|e| e.from.as_deref())
            .find_map(|id| {
                truth
                    .facts
                    .iter()
                    .find(|f| f.id == id && f.kind == "user_text")
            });
        if let (Some(a), Some(b)) = (instruction.cloned(), prompt.cloned())
            && swap_in_views(recon, &a, &b)
        {
            return true;
        }
    }
    false
}

/// Moves an attachment a request carried ahead of that request's instruction, in the trace and session
/// views only: the span keeps the order it was sent in, as a framework would that re-attributes the
/// attachment to an enclosing span.
pub(super) fn move_attachment_before_instruction(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(recorded) = truth.requests.get(&recon.fixture) else {
        return false;
    };
    for request in recorded.calls.values() {
        let instruction = request
            .system
            .iter()
            .filter_map(|o| o.new_fact.as_ref().or(o.replay_of.as_ref()))
            .find_map(|id| truth.facts.iter().find(|f| &f.id == id));
        let attachment = request
            .messages
            .iter()
            .flat_map(|m| m.parts.iter())
            .filter_map(|o| o.new_fact.as_ref())
            .find_map(|id| {
                truth
                    .facts
                    .iter()
                    .find(|f| &f.id == id && f.kind == "user_media")
            });
        let (Some(instruction), Some(attachment)) = (instruction, attachment) else {
            continue;
        };
        let (left, right) = (
            super::mutate::locate(instruction, recon),
            super::mutate::locate(attachment, recon),
        );
        let mut moved = false;
        for &(v, i) in &left {
            if !matches!(recon.views[v].kind, ViewKind::Trace | ViewKind::Session) {
                continue;
            }
            if let Some(&(_, j)) = right.iter().find(|(w, j)| *w == v && *j > i) {
                recon.views[v].blocks.swap(i, j);
                moved = true;
            }
        }
        if moved {
            return true;
        }
    }
    false
}

/// Edits every block a span shows its call was sent that is reasoning the model signed and withheld the
/// text of. Only re-sent copies: the conversation views do not owe withheld reasoning yet, so a response's
/// own block is the withheld-reasoning batch's to hold to account.
pub(super) fn edit_resent_withheld_reasoning(recon: &mut Recon, edit: fn(&mut Block)) -> bool {
    let mut found = false;
    let inputs = recon
        .views
        .iter_mut()
        .filter(|view| view.kind == ViewKind::Span)
        .flat_map(|view| view.blocks.iter_mut())
        .filter(|block| !block.output);
    for block in inputs {
        if block.is("assistant", "thinking")
            && block.text().is_some_and(|text| text.trim().is_empty())
            && block.content.get("signed") == Some(&Value::Bool(true))
        {
            edit(block);
            block.refresh();
            found = true;
        }
    }
    found
}

/// The first composed span view of a fixture, with the index of a block it composed rather than carried itself.
fn composed_block(recon: &Recon) -> Option<(usize, usize)> {
    recon
        .views
        .iter()
        .enumerate()
        .filter(|(_, v)| v.kind == ViewKind::Span && !v.thread.is_empty())
        .find_map(|(view, v)| {
            let block = v.blocks.iter().position(|b| b.span != v.key)?;
            Some((view, block))
        })
}

/// A composed block whose origin is a span the thread has no claim on: the right content from the wrong
/// occurrence, which content identity alone would accept.
pub(super) fn forge_composed_provenance(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((view, block)) = composed_block(recon) else {
        return false;
    };
    recon.views[view].blocks[block].span = "0000000000000000".to_string();
    true
}

/// A composed block kept with its own span, but claiming a carrier that span never wrote - the same content, and
/// no occurrence behind it.
pub(super) fn forge_composed_carrier(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((view, block)) = composed_block(recon) else {
        return false;
    };
    recon.views[view].blocks[block].carrier = "acme.invented".to_string();
    true
}

/// A block only another thread carried, shown by this request: the isolation failure a subagent's thread and its
/// parent's would otherwise hide, since both are one session.
///
/// The other thread is **made** where the fixture has only one, by declaring a span the target's thread does not
/// hold to be a thread of its own. That is the situation a second agent produces, and building it here is what
/// lets the check be exercised by every fixture that composes at all rather than only by a multi-agent capture.
pub(super) fn leak_another_threads_block(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(target) = recon
        .views
        .iter()
        .position(|v| v.kind == ViewKind::Span && !v.thread.is_empty())
    else {
        return false;
    };
    // A span whose own view shows something and which this thread does not hold.
    let other = recon
        .views
        .iter()
        .filter(|v| v.kind == ViewKind::Span)
        .find_map(|v| {
            let block = v.blocks.iter().find(|b| b.span == v.key)?;
            let origin = (block.trace.clone(), block.span.clone());
            (!recon.views[target].thread.contains(&origin)).then(|| (origin, block.clone()))
        });
    let Some((origin, block)) = other else {
        return false;
    };
    // That span is another thread's request, and its block is shown here.
    let mut theirs = recon.views[target].clone();
    theirs.key = origin.1.clone();
    theirs.thread = std::iter::once(origin.clone()).collect();
    theirs.blocks = vec![block.clone()];
    recon.views.push(theirs);
    // Another thread's request frames nothing here: where this request opened with a frame recorded on that span,
    // the mutation's premise is that it is not this request's frame.
    recon.views[target].frames.remove(&origin);
    recon.views[target].blocks.insert(0, block);
    true
}

/// A framed request shows a block of its framing span that no frame recorded for it holds: a block of that span's
/// own view, its text altered, at the head of the request's view. Only the frame records' own occurrences may be
/// shown from a span the request is framed from.
pub(super) fn forge_a_frame_occurrence(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((target, block)) = recon.views.iter().enumerate().find_map(|(target, v)| {
        if v.kind != ViewKind::Span || v.frames.is_empty() {
            return None;
        }
        let block = recon.views.iter().find_map(|framing| {
            (framing.kind == ViewKind::Span).then_some(())?;
            framing.blocks.iter().find(|b| {
                b.span == framing.key && v.frames.contains(&(b.trace.clone(), b.span.clone()))
            })
        })?;
        Some((target, block.clone()))
    }) else {
        return false;
    };
    let mut forged = block;
    let Some(text) = forged.content.get("text").and_then(Value::as_str) else {
        return false;
    };
    forged.content["text"] = Value::String(format!("{text} - and an instruction no record holds"));
    forged.refresh();
    recon.views[target].blocks.insert(0, forged);
    true
}

/// The frame-origin blocks a framed span view shows, a view outside any thread first: `(outside a thread, view,
/// block)`.
fn frame_blocks(recon: &Recon) -> Vec<(bool, usize, usize)> {
    let mut candidates = Vec::new();
    for (at, view) in recon.views.iter().enumerate() {
        if view.kind != ViewKind::Span || view.frames.is_empty() {
            continue;
        }
        for (index, block) in view.blocks.iter().enumerate() {
            let origin = (block.trace.clone(), block.span.clone());
            if block.span != view.key
                && view.frames.contains(&origin)
                && !view.thread.contains(&origin)
                && !view.owned_calls.contains(&origin)
            {
                candidates.push((!view.thread.is_empty(), at, index));
            }
        }
    }
    candidates.sort();
    candidates
}

/// A framed request shows a frame's block at a place no frame record holds it: the right text, span and carrier,
/// read from a message of its record that is not there. Content on the right span is not an occurrence.
pub(super) fn move_a_frame_block(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(&(_, at, index)) = frame_blocks(recon).first() else {
        return false;
    };
    let block = &mut recon.views[at].blocks[index];
    let mut steps: Vec<String> = block.position.split('.').map(str::to_string).collect();
    let Some(message) = steps.get_mut(1) else {
        return false;
    };
    *message = "100000".to_string();
    block.position = steps.join(".");
    true
}

/// A framed request shows a frame's block at a place in its message the record does not hold it: the right record
/// and message, the wrong block of it.
pub(super) fn move_a_frame_block_within_its_message(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(&(_, at, index)) = frame_blocks(recon).first() else {
        return false;
    };
    let block = &mut recon.views[at].blocks[index];
    let Some((head, _)) = block.position.rsplit_once('.') else {
        return false;
    };
    block.position = format!("{head}.100000");
    true
}

/// A framed request shows a frame's block twice where its record holds it once.
pub(super) fn repeat_a_frame_block(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(&(_, at, index)) = frame_blocks(recon).first() else {
        return false;
    };
    let copy = recon.views[at].blocks[index].clone();
    recon.views[at].blocks.insert(index + 1, copy);
    true
}

/// A framed request shows a frame's block from a carrier no frame recorded for it wrote: the right text, on the
/// framing span, from the wrong carrier. Content is not an occurrence, so the block is a provenance failure and
/// nothing else. A framed view outside any thread is preferred, since the frames are all such a view composes.
pub(super) fn relabel_a_frame_carrier(_truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(&(_, at, index)) = frame_blocks(recon).first() else {
        return false;
    };
    recon.views[at].blocks[index].carrier = "acme.invented".to_string();
    true
}
