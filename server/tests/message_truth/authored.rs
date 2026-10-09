//! Text a framework writes into a conversation itself (`evidence: framework`): declared by its suite with the
//! source line that writes it, and proven per capture, so a declaration cannot excuse what the model said,
//! what anyone sent, or what no payload carries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value::Value as Otlp};
use serde_json::Value;

use super::matching::Matching;
use super::recon::Recon;
use super::truth::{Fact, Truth, hex_digest};

/// For each framework-written fact, the spans whose raw payload holds its text byte for byte, written where its
/// place in the conversation says (`placed_holders`): where the views must attribute it.
pub(super) fn emitting_spans(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
) -> BTreeMap<String, BTreeSet<String>> {
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
            let (before, after) = call_starts(truth, fact, recon, matching).unwrap_or_default();
            let placed = placed_holders(&holders(&spans, fact.text()), &before, &after);
            (fact.id.clone(), placed)
        })
        .collect()
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
fn holders(spans: &[RawSpan], text: &str) -> Vec<(String, u64)> {
    spans
        .iter()
        .flat_map(|span| {
            span.strings
                .iter()
                .filter(|(s, _)| s == text)
                .map(|(_, at)| (span.id.clone(), *at))
        })
        .collect()
}

/// The spans whose copy of the text was written where its place in the conversation says: after every call
/// before it started, and before every call after it started.
fn placed_holders(holders: &[(String, u64)], before: &[u64], after: &[u64]) -> BTreeSet<String> {
    holders
        .iter()
        .filter(|(_, at)| before.iter().all(|t| t < at) && after.iter().all(|t| t >= at))
        .map(|(span, _)| span.clone())
        .collect()
}

/// One raw span: its id, and every string its attributes and events hold, JSON decoded, each with when it was
/// written - an event's own time, or the span's end for its attributes.
struct RawSpan {
    id: String,
    strings: Vec<(String, u64)>,
}

fn raw_spans(paths: &[PathBuf]) -> Vec<RawSpan> {
    let mut out = Vec::new();
    for path in paths {
        let request = crate::decode_request(path);
        for span in request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            let mut strings = Vec::new();
            for kv in &span.attributes {
                if let Some(value) = &kv.value {
                    let mut found = Vec::new();
                    collect_otlp(value, &mut found);
                    strings.extend(found.into_iter().map(|s| (s, span.end_time_unix_nano)));
                }
            }
            for event in &span.events {
                for kv in &event.attributes {
                    if let Some(value) = &kv.value {
                        let mut found = Vec::new();
                        collect_otlp(value, &mut found);
                        strings.extend(found.into_iter().map(|s| (s, event.time_unix_nano)));
                    }
                }
            }
            out.push(RawSpan {
                id: hex_digest(&span.span_id),
                strings,
            });
        }
    }
    out
}

fn collect_otlp(value: &AnyValue, out: &mut Vec<String>) {
    match &value.value {
        Some(Otlp::StringValue(s)) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(s)
                && (parsed.is_object() || parsed.is_array())
            {
                collect_json(&parsed, out);
            }
            out.push(s.clone());
        }
        Some(Otlp::ArrayValue(items)) => items.values.iter().for_each(|v| collect_otlp(v, out)),
        Some(Otlp::KvlistValue(list)) => list
            .values
            .iter()
            .filter_map(|kv| kv.value.as_ref())
            .for_each(|v| collect_otlp(v, out)),
        _ => {}
    }
}

fn collect_json(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_json(v, out)),
        Value::Object(members) => members.values().for_each(|v| collect_json(v, out)),
        _ => {}
    }
}

/// Why a framework-authored fact does not hold for one capture, or `None` when it does. It holds where:
/// - its text is, byte for byte, a string of a span's raw payload (`emitting_spans`);
/// - no other fact of the truth, of any kind - a response part the wire recorded, a prompt, a result -
///   holds it;
/// - no recorded request sent it as user or system content;
/// - its place is the one the raw payload's timing says: every call before it started before a span that
///   holds it ended, and every call after it started after that span ended;
/// - every call after it whose request was recorded was sent it.
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
                match refusal(&checked, fixture, fact, &recon, &matching) {
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
    // Shown on a span that did not write it, or listed behind the response it was written after in the feed.
    let mut elsewhere = recon.clone();
    elsewhere.views[v].blocks[b].span = "a-span-that-did-not-write-it".into();
    let new = added(&truth, &elsewhere, &baseline);
    assert!(
        new.contains(&format!("attribution.span:{}", authored.id)),
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
