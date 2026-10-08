//! The edits the mutation catalogue applies: each simulates one parser defect, or one legitimate
//! variation, on a reconstruction (or, for cardinality, on the truth). Each returns whether it found
//! something to edit.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::predicates::{Shows, shows};
use super::recon::{Block, Generation, Recon, ViewKind};
use super::truth::{Fact, Truth};

// --- Locating and editing ----------------------------------------------------------------------

/// Every (view, block) that shows the fact, in every view.
pub(super) fn locate(fact: &Fact, recon: &Recon) -> Vec<(usize, usize)> {
    let call_id = fact.value.get("call_id").and_then(Value::as_str);
    let mut found = Vec::new();
    for (v, view) in recon.views.iter().enumerate() {
        for (b, block) in view.blocks.iter().enumerate() {
            if shows(fact, block, call_id) != Shows::No {
                found.push((v, b));
            }
        }
    }
    found
}

pub(super) fn first_fact<'a>(
    truth: &'a Truth,
    recon: &Recon,
    pick: impl Fn(&Fact) -> bool,
) -> Option<&'a Fact> {
    truth
        .facts
        .iter()
        .filter(|f| f.require.is_some())
        .find(|f| pick(f) && !locate(f, recon).is_empty())
}

pub(super) fn edit_fact(
    truth: &mut Truth,
    recon: &mut Recon,
    pick: fn(&Fact) -> bool,
    edit: fn(&mut Block),
) -> bool {
    let Some(fact) = first_fact(truth, recon, pick) else {
        return false;
    };
    for (v, b) in locate(fact, recon) {
        let block = &mut recon.views[v].blocks[b];
        edit(block);
        block.refresh();
    }
    true
}

pub(super) fn remove_positions(recon: &mut Recon, mut positions: Vec<(usize, usize)>) {
    positions.sort();
    for (v, b) in positions.into_iter().rev() {
        recon.views[v].blocks.remove(b);
    }
}

pub(super) fn remove_fact(truth: &mut Truth, recon: &mut Recon, pick: fn(&Fact) -> bool) -> bool {
    let Some(fact) = first_fact(truth, recon, pick) else {
        return false;
    };
    let positions = locate(fact, recon);
    remove_positions(recon, positions);
    true
}

pub(super) fn set(block: &mut Block, key: &str, value: Value) {
    if let Some(map) = block.content.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

pub(super) fn strip(block: &mut Block, key: &str) {
    if let Some(map) = block.content.as_object_mut() {
        map.remove(key);
    }
}

pub(super) fn flip(text: &str, at: usize) -> String {
    text.char_indices()
        .map(|(i, c)| {
            if i == at {
                if c == 'x' { 'y' } else { 'x' }
            } else {
                c
            }
        })
        .collect()
}

pub(super) fn visible_reasoning(fact: &Fact) -> bool {
    fact.kind == "reasoning" && !fact.text().is_empty()
}

pub(super) fn usage(
    call: &super::truth::Call,
    field: fn(&super::truth::Usage) -> Option<i64>,
) -> bool {
    call.usage.as_ref().and_then(field).is_some_and(|n| n > 0)
}

pub(super) fn edit_text(
    truth: &mut Truth,
    recon: &mut Recon,
    longer_than: usize,
    edit: fn(&str) -> String,
) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| f.kind == "text" && f.call.is_some())
        .filter(|f| f.text().len() > longer_than)
        .or_else(|| {
            truth.facts.iter().find(|f| {
                f.kind == "text"
                    && f.require.is_some()
                    && f.text().len() > longer_than
                    && !locate(f, recon).is_empty()
            })
        })
    else {
        return false;
    };
    for (v, b) in locate(fact, recon) {
        let block = &mut recon.views[v].blocks[b];
        let text = block.text().unwrap_or("").to_string();
        set(block, "text", json!(edit(&text)));
        block.refresh();
    }
    true
}

pub(super) fn alter_media(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| f.kind == "user_media") else {
        return false;
    };
    let positions: Vec<(usize, usize)> = locate(fact, recon)
        .into_iter()
        .filter(|(v, b)| recon.views[*v].blocks[*b].media_sha256.is_some())
        .collect();
    for &(v, b) in &positions {
        let block = &mut recon.views[v].blocks[b];
        let data = block.content["data"].as_str().unwrap_or("").to_string();
        let altered = format!(
            "{}{}",
            if data.starts_with('A') { 'B' } else { 'A' },
            &data[1..]
        );
        set(block, "data", json!(altered));
        block.refresh();
    }
    !positions.is_empty()
}

pub(super) fn alter_argument(truth: &mut Truth, recon: &mut Recon) -> bool {
    edit_fact(
        truth,
        recon,
        |f| f.kind == "tool_call",
        |b| {
            if let Some(input) = b.content.get_mut("input").and_then(Value::as_object_mut)
                && let Some(key) = input.keys().next().cloned()
            {
                input.insert(key, json!("Atlantis"));
            }
        },
    )
}

pub(super) fn call_facts<'a>(truth: &'a Truth, call: &str) -> Vec<&'a Fact> {
    truth
        .facts
        .iter()
        .filter(|f| f.call.as_deref() == Some(call) && f.require.is_some())
        .collect()
}

pub(super) fn delete_call(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(call) = truth
        .calls
        .iter()
        .find(|c| c.succeeded() && !call_facts(truth, &c.id).is_empty())
    else {
        return false;
    };
    let mut positions: Vec<(usize, usize)> = call_facts(truth, &call.id)
        .iter()
        .flat_map(|f| locate(f, recon))
        .collect();
    positions.sort();
    positions.dedup();
    remove_positions(recon, positions);
    true
}

pub(super) fn delete_final_answer(truth: &mut Truth, recon: &mut Recon) -> bool {
    let answers: Vec<String> = truth
        .conversations
        .iter()
        .flat_map(|c| c.final_answers.clone())
        .collect();
    let Some(fact) = truth
        .facts
        .iter()
        .find(|f| answers.contains(&f.id) && f.require.is_some() && !locate(f, recon).is_empty())
    else {
        return false;
    };
    let positions = locate(fact, recon);
    remove_positions(recon, positions);
    true
}

/// Swaps two blocks in every view that holds both.
pub(super) fn swap_in_views(recon: &mut Recon, a: &Fact, b: &Fact) -> bool {
    let (left, right) = (locate(a, recon), locate(b, recon));
    let mut swapped = false;
    for &(v, i) in &left {
        if let Some(&(_, j)) = right.iter().find(|(w, _)| *w == v) {
            recon.views[v].blocks.swap(i, j);
            swapped = true;
        }
    }
    swapped
}

pub(super) fn swap_parts(truth: &mut Truth, recon: &mut Recon) -> bool {
    for call in &truth.calls {
        let facts = call_facts(truth, &call.id);
        if let [a, b, ..] = facts[..] {
            return swap_in_views(recon, a, b);
        }
    }
    false
}

pub(super) fn swap_calls(truth: &mut Truth, recon: &mut Recon) -> bool {
    let edge = truth.edges.iter().find(|e| e.kind == "call_order").cloned();
    let Some(edge) = edge else { return false };
    let (a, b) = (
        edge.before.unwrap_or_default(),
        edge.after.unwrap_or_default(),
    );
    match (call_facts(truth, &a).first(), call_facts(truth, &b).last()) {
        (Some(x), Some(y)) => swap_in_views(recon, x, y),
        _ => false,
    }
}

pub(super) fn result_and_call(truth: &Truth) -> Option<(&Fact, &Fact)> {
    truth
        .edges
        .iter()
        .filter(|e| e.kind == "result_of")
        .find_map(|e| {
            let result = truth
                .facts
                .iter()
                .find(|f| Some(&f.id) == e.from.as_ref())?;
            let call = truth.facts.iter().find(|f| Some(&f.id) == e.to.as_ref())?;
            Some((result, call))
        })
}

pub(super) fn swap_call_and_result(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((result, call)) = result_and_call(truth) else {
        return false;
    };
    swap_in_views(recon, call, result)
}

pub(super) fn swap_session_turns(truth: &mut Truth, recon: &mut Recon) -> bool {
    if truth.topology != "sessions" || truth.conversations.len() < 2 {
        return false;
    }
    let prompt = |conversation: &str| {
        truth
            .facts
            .iter()
            .find(|f| f.kind == "user_text" && f.conversation == conversation)
    };
    match (
        prompt(&truth.conversations[0].id),
        prompt(&truth.conversations[1].id),
    ) {
        (Some(a), Some(b)) => {
            let (left, right) = (locate(a, recon), locate(b, recon));
            let mut swapped = false;
            for &(v, i) in &left {
                if recon.views[v].kind != ViewKind::Session {
                    continue;
                }
                if let Some(&(_, j)) = right.iter().find(|(w, _)| *w == v) {
                    recon.views[v].blocks.swap(i, j);
                    swapped = true;
                }
            }
            swapped
        }
        _ => false,
    }
}

pub(super) fn duplicate(truth: &mut Truth, recon: &mut Recon, pick: fn(&Fact) -> bool) -> bool {
    let Some(fact) = first_fact(truth, recon, pick) else {
        return false;
    };
    let facts: Vec<&Fact> = match &fact.call {
        // A whole response: every asserted output of the call.
        Some(call) if fact.kind != "text" => call_facts(truth, call),
        _ => vec![fact],
    };
    let mut positions: Vec<(usize, usize)> = facts.iter().flat_map(|f| locate(f, recon)).collect();
    positions.sort();
    for (v, b) in positions.into_iter().rev() {
        let copy = recon.views[v].blocks[b].clone();
        recon.views[v].blocks.insert(b + 1, copy);
    }
    true
}

pub(super) fn resend_history(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(prompt) = first_fact(truth, recon, |f| f.kind == "user_text") else {
        return false;
    };
    let Some(answer) = first_fact(truth, recon, |f| f.kind == "text" && f.call.is_some()) else {
        return false;
    };
    let mut copied = false;
    for view in recon.views.iter_mut().filter(|v| v.kind == ViewKind::Trace) {
        let history: Vec<Block> = view
            .blocks
            .iter()
            .filter(|b| shows(prompt, b, None) != Shows::No || shows(answer, b, None) != Shows::No)
            .cloned()
            .collect();
        copied |= !history.is_empty();
        view.blocks.extend(history);
    }
    copied
}

pub(super) fn reuse_call_id(truth: &mut Truth, recon: &mut Recon) -> bool {
    let calls: Vec<&Fact> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_call" && f.require.is_some())
        .collect();
    let [first, second, ..] = calls[..] else {
        return false;
    };
    let reused = first.value["id"].clone();
    for (v, b) in locate(second, recon) {
        let block = &mut recon.views[v].blocks[b];
        set(block, "id", reused.clone());
        block.tool_use_id = reused.as_str().map(str::to_owned);
        block.refresh();
    }
    true
}

pub(super) fn result_to_wrong_call(truth: &mut Truth, recon: &mut Recon) -> bool {
    let calls: Vec<&Fact> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_call" && f.require.is_some())
        .collect();
    let Some((result, call)) = result_and_call(truth) else {
        return false;
    };
    let Some(other) = calls.iter().find(|c| c.id != call.id) else {
        return false;
    };
    let wrong = other.value["id"].clone();
    for (v, b) in locate(result, recon) {
        let block = &mut recon.views[v].blocks[b];
        set(block, "tool_use_id", wrong.clone());
        block.tool_use_id = wrong.as_str().map(str::to_owned);
        block.refresh();
    }
    true
}

/// The truth gains an identical second answer, which the single block shown cannot satisfy twice.
pub(super) fn twin_fact(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| f.kind == "text" && f.call.is_some()).cloned()
    else {
        return false;
    };
    let twin = Fact {
        id: "fact-twin".into(),
        ..fact.clone()
    };
    for call in truth
        .calls
        .iter_mut()
        .filter(|c| Some(&c.id) == fact.call.as_ref())
    {
        call.outputs.push(twin.id.clone());
    }
    for conversation in truth
        .conversations
        .iter_mut()
        .filter(|c| c.id == fact.conversation)
    {
        conversation.sequence.push(twin.id.clone());
    }
    truth.facts.push(twin);
    true
}

/// Edits the span matched to the first call `pick` selects, after confirming the rubric matched it.
pub(super) fn edit_generation(
    truth: &mut Truth,
    recon: &mut Recon,
    pick: fn(&super::truth::Call) -> bool,
    edit: fn(&mut Generation),
) -> bool {
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    let Some(index) = truth
        .calls
        .iter()
        .filter(|c| c.succeeded() && pick(c))
        .find_map(|c| matching.span_of.get(&c.id).copied())
    else {
        return false;
    };
    edit(&mut recon.generations[index]);
    true
}

pub(super) fn remove_from_trace_view(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| f.kind == "text" && f.call.is_some()) else {
        return false;
    };
    let positions: Vec<(usize, usize)> = locate(fact, recon)
        .into_iter()
        .filter(|(v, _)| recon.views[*v].kind == ViewKind::Trace)
        .collect();
    let removed = !positions.is_empty();
    remove_positions(recon, positions);
    removed
}

pub(super) fn wrong_trace(truth: &mut Truth, recon: &mut Recon) -> bool {
    let traces: BTreeSet<String> = recon
        .views
        .iter()
        .filter(|v| v.kind == ViewKind::Trace)
        .map(|v| v.key.clone())
        .collect();
    if traces.len() < 2 {
        return false;
    }
    let Some(fact) = first_fact(truth, recon, |f| f.kind == "user_text") else {
        return false;
    };
    for (v, b) in locate(fact, recon) {
        if recon.views[v].kind != ViewKind::Trace {
            continue;
        }
        let block = &mut recon.views[v].blocks[b];
        let other = traces
            .iter()
            .find(|t| **t != block.trace)
            .cloned()
            .expect("two traces");
        block.trace = other;
    }
    true
}

pub(super) fn reverse_in_feed(truth: &mut Truth, recon: &mut Recon) -> bool {
    for call in &truth.calls {
        let facts = call_facts(truth, &call.id);
        if let [a, b, ..] = facts[..] {
            let (left, right) = (locate(a, recon), locate(b, recon));
            for &(v, i) in &left {
                if recon.views[v].kind == ViewKind::Feed
                    && let Some(&(_, j)) = right.iter().find(|(w, _)| *w == v)
                {
                    recon.views[v].blocks.swap(i, j);
                    return true;
                }
            }
        }
    }
    false
}

/// Adds the span a failed attempt would leave: an ERROR generation naming the truth's error, with
/// assistant output when `speaks`.
pub(super) fn failed_span(truth: &mut Truth, recon: &mut Recon, speaks: bool) -> bool {
    let Some(error) = truth
        .calls
        .iter()
        .find_map(|c| (!c.succeeded()).then(|| c.error.clone()).flatten())
    else {
        return false;
    };
    let Some(template) = recon.generations.first().cloned() else {
        return false;
    };
    let span = "failed-attempt-span".to_string();
    recon.generations.push(Generation {
        label: "trace-1/failed attempt/span-0".into(),
        span: span.clone(),
        typed: true,
        failed: true,
        error: Some(format!("Error code: 500 - {error}")),
        response_id: None,
        raw: None,
        ..template.clone()
    });
    let mut blocks = Vec::new();
    if speaks {
        let mut block = Block {
            role: "assistant".into(),
            kind: "text".into(),
            content: json!({"type": "text", "text": "a reply the failed attempt never sent"}),
            tool_use_id: None,
            trace: template.trace.clone(),
            span: span.clone(),
            output: true,
            finish: None,
            media_sha256: None,
            digest: String::new(),
            identity: String::new(),
            carrier: String::new(),
            position: String::new(),
        };
        block.refresh();
        blocks.push(block);
    }
    recon.views.push(super::recon::View {
        kind: ViewKind::Span,
        key: span,
        blocks,
        thread: Default::default(),
        owned_calls: Default::default(),
    });
    true
}

pub(super) fn suppress_retry(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(at) = truth.calls.iter().position(|c| !c.succeeded()) else {
        return false;
    };
    let Some(retry) = truth.calls.get(at + 1).map(|c| c.id.clone()) else {
        return false;
    };
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    let Some(&index) = matching.span_of.get(&retry) else {
        return false;
    };
    let span = recon.generations.remove(index).span;
    recon
        .views
        .retain(|v| !(v.kind == ViewKind::Span && v.key == span));
    true
}

/// A second span showing exactly the same response as a matched one.
pub(super) fn ambiguous_span(truth: &mut Truth, recon: &mut Recon) -> bool {
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    let Some(&index) = matching.span_of.values().next() else {
        return false;
    };
    let original = recon.generations[index].clone();
    let Some(view) = recon.span_view(&original.span).cloned() else {
        return false;
    };
    let span = format!("{}-copy", original.span);
    recon.generations.push(Generation {
        span: span.clone(),
        label: format!("{}-copy", original.label),
        ..original
    });
    recon.views.push(super::recon::View { key: span, ..view });
    true
}

pub(super) fn terminal_answer_without_result(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(call) = truth
        .facts
        .iter()
        .find(|f| f.kind == "tool_call" && f.value["name"] == "final_answer")
    else {
        return false;
    };
    let id = call.value["id"].as_str().map(str::to_owned);
    let call_id = call.id.clone();
    for view in &mut recon.views {
        view.blocks
            .retain(|b| !(b.is_tool_result() && b.result_call_id().map(str::to_owned) == id));
    }
    // A framework that reports no result for the answer: the truth, declaring none, asserts none.
    let results: Vec<String> = truth
        .edges
        .iter()
        .filter(|e| e.kind == "result_of" && e.to.as_deref() == Some(call_id.as_str()))
        .filter_map(|e| e.from.clone())
        .collect();
    truth.facts.retain(|f| !results.contains(&f.id));
    truth
        .edges
        .retain(|e| !e.from.as_ref().is_some_and(|f| results.contains(f)));
    for conversation in &mut truth.conversations {
        conversation.sequence.retain(|f| !results.contains(f));
    }
    true
}

pub(super) fn extra_content(_: &mut Truth, recon: &mut Recon) -> bool {
    for view in recon.views.iter_mut().filter(|v| v.kind != ViewKind::Span) {
        let Some(trace) = view.blocks.first().map(|b| b.trace.clone()) else {
            continue;
        };
        for (role, text) in [
            ("assistant", "Delegating to the weather agent."),
            ("user", "[framework context]"),
        ] {
            let mut block = Block {
                role: role.into(),
                kind: "text".into(),
                content: json!({"type": "text", "text": text}),
                tool_use_id: None,
                trace: trace.clone(),
                span: "framework-span".into(),
                output: false,
                finish: None,
                media_sha256: None,
                digest: String::new(),
                identity: String::new(),
                carrier: String::new(),
                position: String::new(),
            };
            block.refresh();
            view.blocks.insert(1, block);
        }
    }
    true
}

pub(super) fn reencode_result(
    truth: &mut Truth,
    recon: &mut Recon,
    encode: fn(Value) -> Value,
) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| {
        f.kind == "tool_result" && f.require.as_ref().is_some_and(|r| r.matcher == "semantic")
    }) else {
        return false;
    };
    let value = fact.value["value"].clone();
    for (v, b) in locate(fact, recon) {
        let block = &mut recon.views[v].blocks[b];
        set(block, "content", encode(value.clone()));
        block.refresh();
    }
    true
}

pub(super) fn parallel_results_reordered(truth: &mut Truth, recon: &mut Recon) -> bool {
    let results: Vec<&Fact> = truth
        .edges
        .iter()
        .filter(|e| e.kind == "result_of")
        .filter_map(|e| truth.facts.iter().find(|f| Some(&f.id) == e.from.as_ref()))
        .collect();
    let [a, b, ..] = results[..] else {
        return false;
    };
    swap_in_views(recon, a, b)
}

pub(super) fn rewrite_ids_consistently(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((result, call)) = result_and_call(truth) else {
        return false;
    };
    let rewritten = json!("framework-assigned-id");
    for (fact, key) in [(call, "id"), (result, "tool_use_id")] {
        for (v, b) in locate(fact, recon) {
            let block = &mut recon.views[v].blocks[b];
            set(block, key, rewritten.clone());
            block.tool_use_id = rewritten.as_str().map(str::to_owned);
            block.refresh();
        }
    }
    true
}

fn text_block(role: &str, text: &str, trace: &str, span: &str, output: bool) -> Block {
    let mut block = Block {
        role: role.into(),
        kind: "text".into(),
        content: json!({"type": "text", "text": text}),
        tool_use_id: None,
        trace: trace.into(),
        span: span.into(),
        output,
        finish: None,
        media_sha256: None,
        digest: String::new(),
        identity: String::new(),
        carrier: String::new(),
        position: String::new(),
    };
    block.refresh();
    block
}

/// A system prompt the cassette never recorded is a gap, so showing one is not a defect.
pub(super) fn unknowable_system_prompt(truth: &mut Truth, recon: &mut Recon) -> bool {
    let unrecorded = truth
        .gaps
        .iter()
        .any(|g| g.reason == "request_body_unrecorded");
    let asserted = truth.facts.iter().any(|f| f.kind == "system");
    let shown = recon
        .views
        .iter()
        .flat_map(|v| &v.blocks)
        .any(|b| b.role == "system");
    if !unrecorded || asserted || shown {
        return false;
    }
    for view in recon.views.iter_mut().filter(|v| v.kind != ViewKind::Span) {
        let Some(trace) = view.blocks.first().map(|b| b.trace.clone()) else {
            continue;
        };
        view.blocks.insert(
            0,
            text_block(
                "system",
                "You are a travel assistant.",
                &trace,
                "agent-span",
                false,
            ),
        );
    }
    true
}

pub(super) fn misattribute(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| f.kind == "text" && f.call.is_some()) else {
        return false;
    };
    let mut moved = false;
    for (v, b) in locate(fact, recon) {
        if recon.views[v].kind == ViewKind::Trace {
            recon.views[v].blocks[b].span = "an-enclosing-agent-span".into();
            moved = true;
        }
    }
    moved
}

fn matched_pair(truth: &Truth, recon: &Recon) -> Option<(String, String, usize, usize)> {
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    truth
        .edges
        .iter()
        .filter(|e| e.kind == "call_order")
        .find_map(|e| {
            let (a, b) = (e.before.clone()?, e.after.clone()?);
            let (sa, sb) = (*matching.span_of.get(&a)?, *matching.span_of.get(&b)?);
            Some((a, b, sa, sb))
        })
}

pub(super) fn reorder_call_spans(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((_, _, a, b)) = matched_pair(truth, recon) else {
        return false;
    };
    let earlier = recon.generations[a].start;
    recon.generations[a].start = recon.generations[b].start + chrono::TimeDelta::seconds(1);
    recon.generations[b].start = earlier;
    true
}

pub(super) fn unexpected_generation(_: &mut Truth, recon: &mut Recon) -> bool {
    let Some(template) = recon.generations.iter().find(|g| g.typed).cloned() else {
        return false;
    };
    let span = "unexpected-generation".to_string();
    let block = text_block(
        "assistant",
        "An answer no recorded call gave.",
        &template.trace,
        &span,
        true,
    );
    recon.generations.push(Generation {
        span: span.clone(),
        label: "trace-1/unexpected/span-0".into(),
        response_id: None,
        ..template
    });
    recon.views.push(super::recon::View {
        kind: ViewKind::Span,
        key: span,
        thread: Default::default(),
        owned_calls: Default::default(),
        blocks: vec![block],
    });
    true
}

/// The second call's response moved onto the first call's span: one span now records both.
pub(super) fn share_span(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((_, b_call, a, b)) = matched_pair(truth, recon) else {
        return false;
    };
    let (a_span, b_span) = (
        recon.generations[a].span.clone(),
        recon.generations[b].span.clone(),
    );
    let facts = call_facts(truth, &b_call);
    let Some(bv) = recon
        .views
        .iter()
        .position(|v| v.kind == ViewKind::Span && v.key == b_span)
    else {
        return false;
    };
    let moved: Vec<Block> = recon.views[bv]
        .blocks
        .iter()
        .filter(|blk| blk.output && facts.iter().any(|f| shows(f, blk, None) != Shows::No))
        .cloned()
        .collect();
    recon.views[bv].blocks.retain(|blk| !moved.contains(blk));
    let Some(av) = recon
        .views
        .iter_mut()
        .find(|v| v.kind == ViewKind::Span && v.key == a_span)
    else {
        return false;
    };
    av.blocks.extend(moved.into_iter().map(|mut blk| {
        blk.span = a_span.clone();
        blk
    }));
    true
}

/// A call whose output the truth cannot know still needs its span.
pub(super) fn drop_unknowable_call_span(truth: &mut Truth, recon: &mut Recon) -> bool {
    let mut sink = Vec::new();
    let matching = super::matching::match_calls(truth, recon, &mut sink);
    let Some((call, index)) = truth
        .calls
        .iter()
        .rev()
        .find_map(|c| matching.span_of.get(&c.id).map(|&i| (c.id.clone(), i)))
    else {
        return false;
    };
    let outputs: Vec<String> = truth
        .calls
        .iter()
        .find(|c| c.id == call)
        .map(|c| c.outputs.clone())
        .unwrap_or_default();
    for fact in truth.facts.iter_mut().filter(|f| outputs.contains(&f.id)) {
        fact.require = None;
        truth.gaps.push(super::truth::Gap::for_subject(&fact.id));
    }
    let span = recon.generations.remove(index).span;
    recon
        .views
        .retain(|v| !(v.kind == ViewKind::Span && v.key == span));
    true
}

pub(super) fn leak_prompt(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| f.kind == "user_text") else {
        return false;
    };
    let traces: Vec<usize> = recon
        .views
        .iter()
        .enumerate()
        .filter(|(_, v)| v.kind == ViewKind::Trace)
        .map(|(i, _)| i)
        .collect();
    let Some(&(v, b)) = locate(fact, recon).iter().find(|(v, _)| traces.contains(v)) else {
        return false;
    };
    let Some(&other) = traces.iter().find(|&&t| t != v) else {
        return false;
    };
    let mut copy = recon.views[v].blocks[b].clone();
    copy.trace = recon.views[other].key.clone();
    recon.views[other].blocks.push(copy);
    true
}

pub(super) fn restyle_in_session(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(fact) = first_fact(truth, recon, |f| {
        f.kind == "user_text" && f.require.as_ref().is_some_and(|r| r.matcher == "contains")
    }) else {
        return false;
    };
    let mut edited = false;
    for (v, b) in locate(fact, recon) {
        if recon.views[v].kind == ViewKind::Session {
            let block = &mut recon.views[v].blocks[b];
            let text = format!("{} (edited)", block.text().unwrap_or(""));
            set(block, "text", json!(text));
            block.refresh();
            edited = true;
        }
    }
    edited
}

/// Moves the block showing `fact` to just after the block showing `anchor`, in every trace view.
fn move_after(recon: &mut Recon, fact: &Fact, anchor: &Fact) -> bool {
    let mut moved = false;
    for v in 0..recon.views.len() {
        if recon.views[v].kind != ViewKind::Trace {
            continue;
        }
        let find = |recon: &Recon, f: &Fact| {
            let call_id = f.value.get("call_id").and_then(Value::as_str);
            recon.views[v]
                .blocks
                .iter()
                .position(|b| shows(f, b, call_id) != Shows::No)
        };
        let (Some(from), Some(to)) = (find(recon, fact), find(recon, anchor)) else {
            continue;
        };
        if from > to {
            continue;
        }
        let block = recon.views[v].blocks.remove(from);
        recon.views[v].blocks.insert(to, block);
        moved = true;
    }
    moved
}

pub(super) fn result_after_next_response(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some((a, b, _, _)) = matched_pair(truth, recon) else {
        return false;
    };
    let Some(answer) = call_facts(truth, &b).first().copied() else {
        return false;
    };
    let result = truth
        .edges
        .iter()
        .filter(|e| e.kind == "result_of")
        .find_map(|e| {
            let call = truth.facts.iter().find(|f| Some(&f.id) == e.to.as_ref())?;
            (call.call.as_deref() == Some(a.as_str()))
                .then(|| truth.facts.iter().find(|f| Some(&f.id) == e.from.as_ref()))
                .flatten()
        });
    let Some(result) = result else { return false };
    move_after(recon, result, answer)
}

pub(super) fn prompt_after_response(truth: &mut Truth, recon: &mut Recon) -> bool {
    let Some(edge) = truth.edges.iter().find(|e| e.kind == "prompt_of") else {
        return false;
    };
    let Some(prompt) = truth
        .facts
        .iter()
        .find(|f| Some(&f.id) == edge.from.as_ref())
    else {
        return false;
    };
    let Some(answer) = edge
        .to
        .as_deref()
        .and_then(|c| call_facts(truth, c).first().copied())
    else {
        return false;
    };
    move_after(recon, prompt, answer)
}
