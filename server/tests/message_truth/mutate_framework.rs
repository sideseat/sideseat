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
            };
            block.refresh();
            view.blocks.insert(position, block);
        }
    }
    true
}
