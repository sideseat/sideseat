//! Mutations for what a framework, not the model, puts in a conversation: calls it names itself and
//! the prompt its own state message restates every step.

use serde_json::{Value, json};

use super::mutate::{locate, result_and_call, rewrite_ids_consistently};
use super::recon::{Block, Recon, ViewKind};
use super::truth::{Gap, Truth};

/// A call the wire gave no id, which the framework names: shown under the framework's id it is the call,
/// not a rewrite of one.
pub(super) fn framework_named_call(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((result, call)) = result_and_call(truth) else {
        return false;
    };
    let (result, call) = (result.id.clone(), call.id.clone());
    if !rewrite_ids_consistently(truth, recon) {
        return false;
    }
    for fact in truth.facts.iter_mut() {
        if fact.id == call {
            fact.value["id"] = Value::Null;
        } else if fact.id == result {
            fact.value["call_id"] = Value::Null;
        }
    }
    true
}

/// The prompt restated `copies` times in the trace of a later step of its turn, where one gap says the
/// framework's request for that step restates it once.
pub(super) fn restate_prompt(truth: &mut Truth, recon: &mut Recon, copies: usize) -> bool {
    // A later step of a turn: a call no prompt is sent to, answering the turn's prompt.
    let mut current: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    let mut chosen = None;
    for call in truth.calls.iter().filter(|c| c.succeeded()) {
        let prompted = truth
            .edges
            .iter()
            .find(|e| e.kind == "prompt_of" && e.to.as_deref() == Some(&call.id))
            .and_then(|e| e.from.as_deref());
        match prompted {
            Some(prompt) => {
                current.insert(call.conversation.as_str(), prompt);
            }
            None => {
                if let Some(&prompt) = current.get(call.conversation.as_str()) {
                    chosen = Some((call.id.clone(), prompt.to_string()));
                    break;
                }
            }
        }
    }
    let Some((second, prompt)) = chosen else {
        return false;
    };
    let Some(prompt) = truth.facts.iter().find(|f| f.id == prompt).cloned() else {
        return false;
    };
    let Some(&(view, at)) = locate(&prompt, recon).first() else {
        return false;
    };
    truth.gaps.push(Gap {
        fact: "user_text".to_string(),
        reason: "framework_restates_prompt".to_string(),
        detail: "restated by a test".to_string(),
        subject: Some(second),
        modes: Vec::new(),
        text: None,
        name: None,
    });
    let trace = recon.views[view].blocks[at].trace.clone();
    let text = format!(
        "<user_request>\n{}\n</user_request>\n<step>2</step>",
        prompt.text()
    );
    // A later step's request, so after the trace's last block - first in the feed, newest first.
    for view in recon.views.iter_mut().filter(|v| v.kind != ViewKind::Span) {
        let position = if view.kind == ViewKind::Feed {
            view.blocks.iter().position(|b| b.trace == trace)
        } else {
            view.blocks
                .iter()
                .rposition(|b| b.trace == trace)
                .map(|i| i + 1)
        };
        let Some(position) = position else {
            continue;
        };
        for _ in 0..copies {
            let mut block = Block {
                role: "user".into(),
                kind: "text".into(),
                content: json!({"type": "text", "text": text}),
                tool_use_id: None,
                trace: trace.clone(),
                span: "framework-step".into(),
                output: false,
                finish: None,
                media_sha256: None,
                digest: String::new(),
                identity: String::new(),
                carrier: String::new(),
                position: String::new(),
            };
            block.refresh();
            view.blocks.insert(position, block);
        }
    }
    true
}

/// A proven-absent model waives only what the producer said: a reconstruction stating a model its span's
/// payload never carried is still wrong.
pub(super) fn invented_model_under_a_gap(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(call) = truth
        .calls
        .iter()
        .find(|c| c.succeeded() && c.model.is_some())
        .map(|c| c.id.clone())
    else {
        return false;
    };
    truth.gaps.push(Gap {
        fact: "model".to_string(),
        reason: "metadata_not_exported".to_string(),
        detail: "declared by a test".to_string(),
        subject: Some(call),
        modes: Vec::new(),
        text: None,
        name: None,
    });
    super::mutate::edit_generation(
        truth,
        recon,
        |c| c.model.is_some(),
        |g| {
            g.request_model = Some("an-invented-model".into());
            g.response_model = None;
        },
    )
}

/// A usage gap on the first call whose wire states an input count.
fn gap_the_input_count(truth: &mut Truth) -> bool {
    let Some(call) = truth
        .calls
        .iter()
        .find(|c| c.succeeded() && c.usage.as_ref().is_some_and(|u| u.input.is_some()))
        .map(|c| c.id.clone())
    else {
        return false;
    };
    truth.gaps.push(Gap {
        fact: "usage.input".to_string(),
        reason: "metadata_not_exported".to_string(),
        detail: "declared by a test".to_string(),
        subject: Some(call),
        modes: Vec::new(),
        text: None,
        name: None,
    });
    true
}

/// A gap saying the span misstates a count it states rightly is needed for nothing.
pub(super) fn usage_gap_over_a_right_count(truth: &mut Truth, _recon: &mut Recon) -> bool {
    gap_the_input_count(truth)
}

/// A usage gap waives only the count the producer stated: a reconstruction stating one its span's payload never
/// carried is still wrong.
pub(super) fn invented_usage_under_a_gap(truth: &mut Truth, recon: &mut Recon) -> bool {
    gap_the_input_count(truth)
        && super::mutate::edit_generation(
            truth,
            recon,
            |c| c.usage.as_ref().is_some_and(|u| u.input.is_some()),
            |g| g.input = 987_654_321,
        )
}

/// A usage gap waives a count only under the member the producer stated it in: swapping two counters the span
/// states is still wrong, though each value is in its payload.
pub(super) fn swapped_usage_under_a_gap(truth: &mut Truth, recon: &mut Recon) -> bool {
    if !gap_the_input_count(truth) {
        return false;
    }
    // Both counts gapped, so each swapped value is a count the payload holds - only under the other's name.
    let mut output = truth.gaps[truth.gaps.len() - 1].clone();
    output.fact = "usage.output".to_string();
    truth.gaps.push(output);
    super::mutate::edit_generation(
        truth,
        recon,
        |c| c.usage.as_ref().is_some_and(|u| u.input.is_some()),
        |g| std::mem::swap(&mut g.input, &mut g.output),
    )
}

/// The span's output count reconstructed as a value planted in its raw payload under `key`.
fn plant_output(g: &mut super::recon::Generation, key: &str, value: serde_json::Value) {
    const PLANTED: i64 = 6_000_123;
    let mut raw: serde_json::Value = g
        .raw
        .as_deref()
        .and_then(|r| serde_json::from_str(r).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    // Planted among the span's attributes, where a producer states its counts.
    if let Some(attributes) = raw
        .as_object_mut()
        .map(|map| {
            map.entry("attributes")
                .or_insert_with(|| serde_json::json!({}))
        })
        .and_then(serde_json::Value::as_object_mut)
    {
        attributes.insert(key.to_string(), value);
    }
    g.raw = Some(raw.to_string());
    g.output = PLANTED;
}

/// Only the output count gapped, so nothing but the value taken for it can speak.
fn gap_the_output_count(truth: &mut Truth) -> bool {
    if !gap_the_input_count(truth) {
        return false;
    }
    if let Some(gap) = truth.gaps.last_mut() {
        gap.fact = "usage.output".to_string();
    }
    true
}

fn output_picked(c: &super::truth::Call) -> bool {
    c.usage.as_ref().is_some_and(|u| u.input.is_some())
}

/// A request's limit is no usage count: a gapped count reconstructed as the span's `max_output_tokens`, or as an
/// `output_tokens` under its request, is still wrong, though that value is in the span's payload.
pub(super) fn limit_for_usage_under_a_gap(truth: &mut Truth, recon: &mut Recon) -> bool {
    gap_the_output_count(truth)
        && super::mutate::edit_generation(truth, recon, output_picked, |g| {
            plant_output(g, "max_output_tokens", serde_json::json!(6_000_123));
        })
}

pub(super) fn nested_limit_for_usage_under_a_gap(truth: &mut Truth, recon: &mut Recon) -> bool {
    gap_the_output_count(truth)
        && super::mutate::edit_generation(truth, recon, output_picked, |g| {
            plant_output(
                g,
                "request",
                serde_json::json!({"output_tokens": 6_000_123}),
            );
        })
}

/// A prompt's text is no usage count: a message body quoting `output_tokens` is not the span stating one.
pub(super) fn prompt_for_usage_under_a_gap(truth: &mut Truth, recon: &mut Recon) -> bool {
    gap_the_output_count(truth)
        && super::mutate::edit_generation(truth, recon, output_picked, |g| {
            plant_output(
                g,
                "gen_ai.input.messages",
                serde_json::json!(
                    serde_json::json!([{"role": "user", "content": "{\"output_tokens\": 6000123}"}])
                        .to_string()
                ),
            );
        })
}

/// Another counter is no output count, however its name is spelt: reasoning tokens under a dotted name are still
/// reasoning tokens.
pub(super) fn reasoning_for_output_under_a_gap(truth: &mut Truth, recon: &mut Recon) -> bool {
    gap_the_output_count(truth)
        && super::mutate::edit_generation(truth, recon, output_picked, |g| {
            plant_output(
                g,
                "gen_ai.usage.reasoning.output_tokens",
                serde_json::json!(6_000_123),
            );
        })
}
