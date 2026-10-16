//! Text a framework writes into a conversation itself (`evidence: framework`): declared by its suite with the
//! source line that writes it, and proven per capture, so a declaration cannot excuse what the model said,
//! what anyone sent, or what no payload carries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

use super::matching::Matching;
use super::recon::Recon;
use super::truth::{Fact, Truth, hex_digest};
use raw::{RawSpan, raw_spans};

// The proof of a reply the framework keeps from the model, tested apart to keep this file short.
mod kept_from_model;
mod raw;
mod returned_result;

/// For each framework-written fact, the spans whose raw payload holds its text byte for byte, written where its
/// place in the conversation says (`placed_holders`): where the views must attribute it.
pub(super) fn emitting_spans(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
) -> BTreeMap<String, BTreeSet<(String, String)>> {
    let authored: Vec<&Fact> = truth
        .facts
        .iter()
        .filter(|f| f.evidence == "framework")
        .collect();
    if authored.is_empty() {
        return BTreeMap::new();
    }
    let spans = raw_spans(&recon.paths);
    authored
        .into_iter()
        .map(|fact| {
            if fact.kind == "user_text" {
                // A prompt is the request of the call it was sent to, so that call's span is where it is shown.
                let held = prompted_span(truth, fact, recon, matching)
                    .filter(|at| span_holds(recon, at, fact.text()));
                return (fact.id.clone(), held.into_iter().collect());
            }
            let (before, after) = call_starts(truth, fact, recon, matching).unwrap_or_default();
            let placed = placed_holders(&holders(&spans, fact.text()), &before, &after);
            (fact.id.clone(), placed)
        })
        .collect()
}

/// The one call a framework prompt was sent to (`prompt_of`), or `None` where it names none or several.
fn prompted_call<'t>(truth: &'t Truth, fact: &Fact) -> Option<&'t str> {
    let mut calls = truth
        .edges
        .iter()
        .filter(|e| e.kind == "prompt_of" && e.from.as_deref() == Some(fact.id.as_str()))
        .filter_map(|e| e.to.as_deref());
    match (calls.next(), calls.next()) {
        (Some(call), None) => Some(call),
        _ => None,
    }
}

/// The span of the call a framework prompt was sent to, as `(trace, span)`: span ids repeat across traces.
fn prompted_span(
    truth: &Truth,
    fact: &Fact,
    recon: &Recon,
    matching: &Matching,
) -> Option<(String, String)> {
    let call = prompted_call(truth, fact)?;
    let generation = &recon.generations[matching.span_showing(call)?];
    Some((generation.trace.clone(), generation.span.clone()))
}

/// Whether the span's payload holds the text byte for byte as one whole string: an attribute's, one inside an
/// attribute that is JSON, or one inside a serialised value the payload decodes to (a Python repr, say).
fn span_holds(recon: &Recon, (trace, span): &(String, String), text: &str) -> bool {
    fn within(value: &Value, text: &str) -> bool {
        match value {
            Value::String(s) => s == text,
            Value::Array(items) => items.iter().any(|v| within(v, text)),
            Value::Object(members) => members.values().any(|v| within(v, text)),
            _ => false,
        }
    }
    if raw_spans(&recon.paths)
        .iter()
        .filter(|raw| raw.trace == *trace && raw.id == *span)
        .any(|raw| raw.strings.iter().any(|(s, _)| s == text))
    {
        return true;
    }
    super::absence::haystack::Haystack::of_fixture(&recon.paths)
        .carriers
        .iter()
        .filter(|c| {
            c.trace.as_deref() == Some(trace.as_str()) && c.span.as_deref() == Some(span.as_str())
        })
        .any(|c| c.nodes.iter().any(|(_, node)| within(node, text)))
}

/// Whether a request's text holds the prompt as one of its own lines' runs: the whole text, or the text at a
/// line boundary at either end, as a framework that joins its prompt to the message before it writes it.
fn sends_prompt(sent: &str, prompt: &str) -> bool {
    sent == prompt
        || sent.ends_with(&format!("\n{prompt}"))
        || sent.starts_with(&format!("{prompt}\n"))
        || sent.contains(&format!("\n{prompt}\n"))
}

/// Why a framework prompt (`user_text` on `framework` evidence) does not hold for one capture, or `None` when it
/// does. It holds where:
/// - it names exactly one call it was sent to (`prompt_of`);
/// - that call's span holds it, byte for byte, as a whole string of its raw payload;
/// - no other fact of the truth holds it, so no model said it and no script sent it;
/// - the capture's transcript records that call's request, and a user message of it sends it byte for byte, alone
///   or joined at a line boundary to the message before it; and no earlier call's recorded request sent it.
pub(super) fn prompt_refusal(
    truth: &Truth,
    fixture: &str,
    fact: &Fact,
    recon: &Recon,
    matching: &Matching,
) -> Option<String> {
    let text = fact.text();
    if fact.call.is_some() || fact.role != "user" || text.is_empty() {
        return Some(format!("{} is not a user text outside every call", fact.id));
    }
    let Some(call) = prompted_call(truth, fact) else {
        return Some(format!("{} names no one call it was sent to", fact.id));
    };
    let Some(span) = prompted_span(truth, fact, recon, matching) else {
        return Some(format!("{call} has no span"));
    };
    if !span_holds(recon, &span, text) {
        return Some(format!(
            "{call}'s span does not hold {} as written",
            fact.id
        ));
    }
    if let Some(other) = truth
        .facts
        .iter()
        .filter(|f| f.id != fact.id)
        .find(|f| holds_text(&f.value, text))
    {
        return Some(format!("{} holds it", other.id));
    }
    // The request is the evidence that the prompt was sent: a capture without its transcript, or a transcript
    // without that call's request, proves nothing about it.
    let Some(recorded) = truth.requests.get(fixture) else {
        return Some(format!(
            "{fixture} records no request, so nothing shows {call} was sent it"
        ));
    };
    let user_texts = |request: &super::truth::CallRequest| -> Vec<String> {
        request
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .flat_map(|m| &m.parts)
            .filter_map(|o| o.part["text"].as_str().map(str::to_string))
            .collect()
    };
    let Some(request) = recorded.calls.get(call) else {
        return Some(format!("no recorded request is {call}'s"));
    };
    if !user_texts(request)
        .iter()
        .any(|sent| sends_prompt(sent, text))
    {
        return Some(format!(
            "{call}'s request did not send {} as written",
            fact.id
        ));
    }
    // First sent to this call: every earlier call that answered had its request recorded, and none sent it. An
    // earlier request missing from the transcript could have, so it refuses the proof rather than passing it.
    for early in truth
        .calls
        .iter()
        .take_while(|c| c.id != call)
        .filter(|c| c.succeeded())
    {
        let Some(request) = recorded.calls.get(&early.id) else {
            return Some(format!(
                "{}, before {call}, has no recorded request",
                early.id
            ));
        };
        if user_texts(request).iter().any(|sent| sent.contains(text)) {
            return Some(format!("{}, before {call}, was already sent it", early.id));
        }
    }
    None
}

/// When the calls before and after a fact in its conversation started, or `None` where a call has no span.
fn call_starts(
    truth: &Truth,
    fact: &Fact,
    recon: &Recon,
    matching: &Matching,
) -> Option<(Vec<u64>, Vec<u64>)> {
    let conversation = truth
        .conversations
        .iter()
        .find(|c| c.id == fact.conversation)?;
    let at = conversation.sequence.iter().position(|id| *id == fact.id)?;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for call in truth
        .calls
        .iter()
        .filter(|c| c.conversation == fact.conversation && c.succeeded() && !c.outputs.is_empty())
    {
        let generation = &recon.generations[matching.span_showing(&call.id)?];
        let start = generation.start.timestamp_nanos_opt()? as u64;
        let earlier = call
            .outputs
            .iter()
            .any(|o| conversation.sequence.iter().position(|id| id == o) < Some(at));
        if earlier {
            before.push(start);
        } else {
            after.push(start);
        }
    }
    Some((before, after))
}

/// Where the text is held as an attribute's whole string, or a string anywhere inside an attribute that is
/// JSON: each holding span with when that copy was written.
/// The spans, by trace and id, that state the text, as written, as their own output: in one of their output
/// carriers, whole or as a content member of an assistant's message inside it.
fn holders(spans: &[RawSpan], text: &str) -> Vec<((String, String), u64)> {
    spans
        .iter()
        .flat_map(|span| {
            span.outputs
                .iter()
                .filter(|(s, _)| s == text)
                .map(|(_, at)| ((span.trace.clone(), span.id.clone()), *at))
        })
        .collect()
}

/// The spans whose copy of the text was written where its place in the conversation says: after every call
/// before it started, and before every call after it started.
fn placed_holders(
    holders: &[((String, String), u64)],
    before: &[u64],
    after: &[u64],
) -> BTreeSet<(String, String)> {
    holders
        .iter()
        .filter(|(_, at)| before.iter().all(|t| t < at) && after.iter().all(|t| t >= at))
        .map(|(span, _)| span.clone())
        .collect()
}

/// Why a framework-authored fact does not hold for one capture, or `None` when it does. It holds where:
/// - its text is, byte for byte, a string of a span's raw payload (`emitting_spans`);
/// - no other fact of the truth, of any kind - a response part the wire recorded, a prompt, a result -
///   holds it;
/// - no recorded request sent it as user or system content;
/// - its place is the one the raw payload's timing says: every call before it started before a span that
///   holds it ended, and every call after it started after that span ended;
/// - every call after it whose request was recorded was sent it - or, for a reply the framework keeps from the
///   model (`kept_from_model`), no recorded request was, under its lineage or as written.
pub(super) fn refusal(
    truth: &Truth,
    fixture: &str,
    fact: &Fact,
    recon: &Recon,
    matching: &Matching,
) -> Option<String> {
    let text = fact.text();
    if fact.call.is_some() || fact.kind != "text" || fact.role != "assistant" || text.is_empty() {
        return Some(format!(
            "{} is not an assistant text outside every call",
            fact.id
        ));
    }
    let raw = raw_spans(&recon.paths);
    let held = holders(&raw, text);
    if held.is_empty() {
        return Some(format!("no span's payload holds {} as written", fact.id));
    }
    if let Some(other) = truth
        .facts
        .iter()
        .filter(|f| f.id != fact.id)
        .find(|f| holds_text(&f.value, text))
    {
        return Some(format!("{} holds it", other.id));
    }
    let recorded = truth.requests.get(fixture);
    if let Some(call) = recorded
        .into_iter()
        .flat_map(|r| &r.calls)
        .find_map(|(call, request)| {
            let sent = request.system.iter().chain(
                request
                    .messages
                    .iter()
                    .filter(|m| m.role == "user" || m.role == "system")
                    .flat_map(|m| &m.parts),
            );
            sent.into_iter()
                .any(|o| holds_text(&o.part, text))
                .then_some(call)
        })
    {
        return Some(format!(
            "{call}'s request sent it as user or system content"
        ));
    }
    let conversation = truth
        .conversations
        .iter()
        .find(|c| c.id == fact.conversation)?;
    let at = conversation.sequence.iter().position(|id| *id == fact.id)?;
    let after: Vec<&str> = truth
        .calls
        .iter()
        .filter(|c| c.conversation == fact.conversation && c.succeeded() && !c.outputs.is_empty())
        .filter(|c| {
            c.outputs
                .iter()
                .all(|o| conversation.sequence.iter().position(|id| id == o) > Some(at))
        })
        .map(|c| c.id.as_str())
        .collect();
    let Some((before_starts, after_starts)) = call_starts(truth, fact, recon, matching) else {
        return Some("a call of its conversation has no span to time it by".to_string());
    };
    if placed_holders(&held, &before_starts, &after_starts).is_empty() {
        return Some(format!(
            "no copy of {} was written where its place in the conversation says",
            fact.id
        ));
    }
    if fact.kept_from_model {
        return carried_anywhere(fact, recorded);
    }
    for call in after {
        let Some(request) = recorded.and_then(|r| r.calls.get(call)) else {
            continue;
        };
        let carried = request.messages.iter().flat_map(|m| &m.parts).any(|o| {
            [&o.new_fact, &o.replay_of]
                .iter()
                .any(|l| l.as_deref() == Some(fact.id.as_str()))
        });
        if !carried {
            return Some(format!("{call}'s request was not sent it"));
        }
    }
    None
}

/// Why a reply kept from the model is refused by the recorded requests, if it is: one carries it anyhow - as its
/// lineage (a part stating, replaying or rendering it), or as written, whitespace aside, in any part or in the
/// text of the system content or of one message read whole, however its parts split it. Every recorded request
/// counts, whatever conversation or order: a reply the framework keeps from the model is in none, and one an
/// earlier request held was no reply the framework wrote.
fn carried_anywhere(
    fact: &Fact,
    recorded: Option<&super::truth::FixtureRequests>,
) -> Option<String> {
    let text = unspaced(fact.text());
    let names = |o: &super::truth::Occurrence| {
        [&o.new_fact, &o.replay_of]
            .iter()
            .any(|l| l.as_deref() == Some(fact.id.as_str()))
            || o.renders.contains(&fact.id)
    };
    recorded
        .into_iter()
        .flat_map(|r| &r.calls)
        .find(|(_, request)| {
            std::iter::once(&request.system)
                .chain(request.messages.iter().map(|m| &m.parts))
                .any(|parts| {
                    // Read whole, whitespace aside, so a reply split across parts - mid-word, between words, or
                    // both - is still the reply.
                    let mut whole = String::new();
                    for part in parts {
                        strings(&part.part, &mut whole);
                    }
                    parts
                        .iter()
                        .any(|o| names(o) || holds_unspaced(&o.part, &text))
                        || unspaced(&whole).contains(&text)
                })
        })
        .map(|(call, _)| {
            format!("{call}'s request was sent it, which a reply kept from the model is not")
        })
}

/// The texts a part holds, in order: the part itself where it is a string, and the value of every `text` member
/// at any depth - a block's text, not its type or other members, which would sit between two halves of a split
/// reply.
fn strings(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::String(s) => out.push_str(s),
        serde_json::Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        serde_json::Value::Object(members) => {
            for (key, member) in members {
                if key == "text" || member.is_array() || member.is_object() {
                    strings(member, out);
                }
            }
        }
        _ => {}
    }
}

/// The text without its whitespace, so a copy re-wrapped or split at a word break still reads as the text.
fn unspaced(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Whether any string inside the value holds the text, whitespace aside on both sides.
fn holds_unspaced(value: &serde_json::Value, unspaced_text: &str) -> bool {
    match value {
        serde_json::Value::String(s) => unspaced(s).contains(unspaced_text),
        serde_json::Value::Array(items) => items.iter().any(|v| holds_unspaced(v, unspaced_text)),
        serde_json::Value::Object(members) => {
            members.values().any(|v| holds_unspaced(v, unspaced_text))
        }
        _ => false,
    }
}

/// Whether any string inside the value - a text, an argument, a nested member - contains the text.
fn holds_text(value: &serde_json::Value, text: &str) -> bool {
    match value {
        serde_json::Value::String(s) => s.contains(text),
        serde_json::Value::Array(items) => items.iter().any(|v| holds_text(v, text)),
        serde_json::Value::Object(members) => members.values().any(|v| holds_text(v, text)),
        _ => false,
    }
}

#[test]
fn every_framework_authored_fact_is_proven() {
    let truths = super::truth::load_all();
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let mut defects = Vec::new();
    let mut proven = 0;
    for (key, truth) in &truths {
        for fixture in &truth.fixtures {
            let Some(paths) = fixtures.get(fixture) else {
                continue;
            };
            let checked = truth.for_fixture(fixture);
            let authored: Vec<&Fact> = checked
                .facts
                .iter()
                .filter(|f| f.evidence == "framework")
                .collect();
            if authored.is_empty() {
                continue;
            }
            let recon = super::recon::build(fixture, paths);
            let matching = super::matching::match_calls(&checked, &recon, &mut Vec::new());
            for fact in authored {
                let refused = if fact.kind == "user_text" {
                    prompt_refusal(&checked, fixture, fact, &recon, &matching)
                } else {
                    refusal(&checked, fixture, fact, &recon, &matching)
                };
                match refused {
                    None => proven += 1,
                    Some(why) => defects.push(format!("{key}: {} on {fixture}: {why}", fact.id)),
                }
            }
        }
    }
    assert!(
        defects.is_empty(),
        "framework-authored facts without a proof ({proven} proven):\n  {}",
        defects.join("\n  ")
    );
}

/// The proof refuses a text no payload holds as written, one a response or a prompt holds, one a request sent
/// as user content, one placed where the raw timing says it was not written, and one a later request was not
/// sent.
#[test]
fn a_framework_text_is_refused_where_it_does_not_hold() {
    let fixture = "agentscope/sdk/structured_output";
    let paths = crate::discover_fixtures()
        .into_iter()
        .find(|(label, _)| label == fixture)
        .map(|(_, paths)| paths)
        .expect("the capture");
    let truth = super::truth::load_all()["agentscope/structured_output"].for_fixture(fixture);
    let recon = super::recon::build(fixture, &paths);
    let matching = super::matching::match_calls(&truth, &recon, &mut Vec::new());
    let authored = truth
        .facts
        .iter()
        .find(|f| f.evidence == "framework")
        .expect("a framework text")
        .clone();
    let refused = |truth: &Truth, fact: &Fact, recon: &Recon, matching: &Matching| {
        refusal(truth, fixture, fact, recon, matching).is_some()
    };
    assert_eq!(refusal(&truth, fixture, &authored, &recon, &matching), None);
    let mut altered = authored.clone();
    altered.value["text"] = "The required structured output is generated!".into();
    assert!(
        refused(&truth, &altered, &recon, &matching),
        "a text no payload holds"
    );
    for kind in ["text", "user_text"] {
        let mut held = truth.clone();
        let other = held
            .facts
            .iter_mut()
            .find(|f| {
                if kind == "text" {
                    f.call.is_some()
                } else {
                    f.kind == kind
                }
            })
            .expect("a fact to hold it");
        other.value = serde_json::json!({"text": format!("Done. {}", authored.text())});
        assert!(
            refused(&held, &authored, &recon, &matching),
            "a {kind} fact holds it"
        );
    }
    let mut sent = truth.clone();
    let request = sent.requests.get_mut(fixture).expect("a recorded request");
    let first = request.calls.values_mut().next().expect("a request");
    let part = first
        .messages
        .iter_mut()
        .find(|m| m.role == "user")
        .and_then(|m| m.parts.first_mut())
        .expect("a user part");
    part.part = serde_json::json!({"type": "text", "text": authored.text()});
    assert!(
        refused(&sent, &authored, &recon, &matching),
        "a request sent it as user content"
    );
    // Placed before the call it was written after.
    let mut early = truth.clone();
    let sequence = &mut early.conversations[0].sequence;
    sequence.retain(|id| *id != authored.id);
    sequence.insert(0, authored.id.clone());
    assert!(
        refused(&early, &authored, &recon, &matching),
        "placed before its call"
    );
    // A later call, started after the span that wrote it ended, whose recorded request was not sent it.
    let mut later = truth.clone();
    let mut call = later.calls[0].clone();
    call.id = "call-later".into();
    call.outputs = vec!["fact-later".into()];
    let mut output = later
        .facts
        .iter()
        .find(|f| f.call.is_some())
        .expect("a response part")
        .clone();
    output.id = "fact-later".into();
    output.call = Some("call-later".into());
    later.conversations[0].sequence.push("fact-later".into());
    later.facts.push(output);
    later.calls.push(call);
    let request = later.requests.get_mut(fixture).expect("a recorded request");
    let unsent = request.calls.values().next().expect("a request").clone();
    request.calls.insert("call-later".into(), unsent);
    let mut afterwards = recon.clone();
    let mut generation = recon.generations[matching.span_of["call-001"]].clone();
    generation.span = "a-later-span".into();
    generation.start += chrono::Duration::hours(1);
    afterwards.generations.push(generation);
    let mut matched = super::matching::match_calls(&truth, &recon, &mut Vec::new());
    matched
        .span_of
        .insert("call-later".into(), afterwards.generations.len() - 1);
    assert!(
        refused(&later, &authored, &afterwards, &matched),
        "a later request not sent it"
    );
}

/// A declared framework text explains its one block and nothing else: an invented assistant text beside it is
/// still unexplained, and the text missing or shown twice is caught like any fact's.
#[test]
fn a_framework_text_explains_only_its_own_block() {
    use super::mutate::{locate, remove_positions};
    use super::mutations::{added, baseline};
    use super::recon::ViewKind;
    let (truth, recon, baseline) = baseline("agentscope/sdk/structured_output");
    let authored = truth
        .facts
        .iter()
        .find(|f| f.evidence == "framework")
        .expect("a framework text");
    let in_trace: Vec<(usize, usize)> = locate(authored, &recon)
        .into_iter()
        .filter(|&(v, _)| recon.views[v].kind == ViewKind::Trace)
        .collect();
    let &(v, b) = in_trace.first().expect("the trace view shows it");
    let mut invented = recon.clone();
    let mut block = invented.views[v].blocks[b].clone();
    block.content["text"] = "The required structured output is generated!".into();
    block.refresh();
    invented.views[v].blocks.insert(b + 1, block);
    let new = added(&truth, &invented, &baseline);
    assert!(
        new.iter().any(|a| a.starts_with("extra.unexplained:")),
        "an invented text was explained: {new:?}"
    );
    let mut twice = recon.clone();
    let copy = twice.views[v].blocks[b].clone();
    twice.views[v].blocks.insert(b + 1, copy);
    let new = added(&truth, &twice, &baseline);
    assert!(
        new.contains(&format!("text.duplicated:{}", authored.id)),
        "a second copy passed: {new:?}"
    );
    // Shown on a span that did not write it - which owes it as missing there - or listed behind the response it was
    // written after in the feed.
    let mut elsewhere = recon.clone();
    elsewhere.views[v].blocks[b].span = "a-span-that-did-not-write-it".into();
    let new = added(&truth, &elsewhere, &baseline);
    assert!(
        new.contains(&format!(
            "extra.unexplained:{}",
            elsewhere.views[v].blocks[b].digest
        )),
        "the text on another span passed: {new:?}"
    );
    let mut last = recon.clone();
    for view in last.views.iter_mut().filter(|v| v.kind == ViewKind::Feed) {
        if let Some(at) = view
            .blocks
            .iter()
            .position(|b| b.text() == Some(authored.text()))
        {
            let block = view.blocks.remove(at);
            view.blocks.push(block);
        }
    }
    let new = added(&truth, &last, &baseline);
    assert!(
        new.iter()
            .any(|a| a.starts_with("order.sequence:") && a.contains(&authored.id)),
        "the text behind its call in the feed passed: {new:?}"
    );
    let mut missing = recon.clone();
    remove_positions(&mut missing, in_trace);
    let new = added(&truth, &missing, &baseline);
    assert!(
        new.contains(&format!("text.missing:{}", authored.id)),
        "the text missing passed: {new:?}"
    );
}

/// The prompt proof refuses a near miss - one character off - a prompt sent to a call whose span and request do not
/// hold it, one a script fact also holds, and one an earlier request already sent; and accepts it on the old
/// release whose span holds it only inside a serialised value.
#[test]
fn a_framework_prompt_is_refused_where_it_does_not_hold() {
    let load = |fixture: &str| {
        let paths = crate::discover_fixtures()
            .into_iter()
            .find(|(label, _)| label == fixture)
            .map(|(_, paths)| paths)
            .expect("the capture");
        let truth = super::truth::load_all()["smolagents/max_steps"].for_fixture(fixture);
        let recon = super::recon::build(fixture, &paths);
        let matching = super::matching::match_calls(&truth, &recon, &mut Vec::new());
        (truth, recon, matching)
    };
    let fixture = "smolagents/native/max_steps";
    let (truth, recon, matching) = load(fixture);
    let prompt = truth
        .facts
        .iter()
        .find(|f| f.evidence == "framework" && f.kind == "user_text")
        .expect("a framework prompt")
        .clone();
    let refused =
        |truth: &Truth, fact: &Fact| prompt_refusal(truth, fixture, fact, &recon, &matching);
    assert_eq!(refused(&truth, &prompt), None);
    let (old, old_recon, old_matching) = load("smolagents/native@0.1.26/max_steps");
    assert_eq!(
        prompt_refusal(
            &old,
            "smolagents/native@0.1.26/max_steps",
            &prompt,
            &old_recon,
            &old_matching
        ),
        None,
        "a serialised copy of the request holds it"
    );
    let mut near = prompt.clone();
    near.value["text"] = prompt
        .text()
        .replacen("Based on the above,", "Based on the above:", 1)
        .into();
    assert!(refused(&truth, &near).is_some(), "a near miss");
    let mut misplaced = truth.clone();
    for edge in misplaced
        .edges
        .iter_mut()
        .filter(|e| e.kind == "prompt_of" && e.from.as_deref() == Some(prompt.id.as_str()))
    {
        edge.to = Some("call-001".into());
    }
    assert!(
        refused(&misplaced, &prompt).is_some(),
        "sent to the wrong call"
    );
    let mut held = truth.clone();
    let task = held
        .facts
        .iter_mut()
        .find(|f| f.kind == "user_text" && f.evidence == "script")
        .expect("the task");
    task.value["text"] = prompt.text().into();
    assert!(refused(&held, &prompt).is_some(), "a script fact holds it");
    let mut early = truth.clone();
    let request = early.requests.get_mut(fixture).expect("a recorded request");
    let first = request
        .calls
        .get_mut("call-001")
        .expect("the first request");
    let part = first
        .messages
        .iter_mut()
        .find(|m| m.role == "user")
        .and_then(|m| m.parts.first_mut())
        .expect("a user part");
    part.part = serde_json::json!({"type": "text", "text": prompt.text()});
    assert!(
        refused(&early, &prompt).is_some(),
        "an earlier request sent it"
    );
    let mut untranscribed = truth.clone();
    untranscribed.requests.clear();
    assert!(refused(&untranscribed, &prompt).is_some(), "no transcript");
    let mut unrequested = truth.clone();
    unrequested
        .requests
        .get_mut(fixture)
        .expect("a recorded request")
        .calls
        .remove("call-002");
    assert!(
        refused(&unrequested, &prompt).is_some(),
        "no request of its call"
    );
    let mut first_unrecorded = truth.clone();
    first_unrecorded
        .requests
        .get_mut(fixture)
        .expect("a recorded request")
        .calls
        .remove("call-001");
    assert!(
        refused(&first_unrecorded, &prompt).is_some(),
        "an earlier request missing from the transcript"
    );
    // The same span id in another trace is another span: its copy is no evidence for this call's.
    let mut elsewhere = super::matching::match_calls(&truth, &recon, &mut Vec::new());
    let mut moved = recon.clone();
    let at = matching
        .span_showing("call-002")
        .expect("the fallback's span");
    let mut other = moved.generations[at].clone();
    other.trace = "another-trace".into();
    moved.generations.push(other);
    elsewhere
        .span_of
        .insert("call-002".into(), moved.generations.len() - 1);
    assert!(
        prompt_refusal(&truth, fixture, &prompt, &moved, &elsewhere).is_some(),
        "the span id of another trace"
    );
}
