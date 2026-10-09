//! Text a framework writes into a conversation itself (`evidence: framework`): declared by its suite with the
//! source line that writes it, and proven per capture, so a declaration cannot excuse what the model said or
//! what no payload carries.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::absence::haystack::{Haystack, collapse_whitespace};
use super::truth::{Fact, Truth};

/// Why a framework-authored fact does not hold for one capture, or `None` when it does: its text is a whole
/// string of the producer's own payload, as written; no response of the scenario holds it; and every later
/// call of its conversation whose request was recorded was sent it.
pub(super) fn refusal(
    truth: &Truth,
    fixture: &str,
    fact: &Fact,
    haystack: &Haystack,
) -> Option<String> {
    let text = fact.text();
    if fact.call.is_some() || fact.kind != "text" || fact.role != "assistant" || text.is_empty() {
        return Some(format!(
            "{} is not an assistant text outside every call",
            fact.id
        ));
    }
    let collapsed = collapse_whitespace(text);
    let verbatim = haystack
        .carriers
        .iter()
        .filter(|c| c.span.is_some())
        .any(|c| c.strings.iter().any(|(_, s)| *s == collapsed));
    if !verbatim {
        return Some(format!("no span's payload holds {:?} as written", fact.id));
    }
    if let Some(output) = truth
        .facts
        .iter()
        .filter(|f| f.call.is_some())
        .find(|f| holds_text(&f.value, text))
    {
        return Some(format!("the model's response {} holds it", output.id));
    }
    let conversation = truth
        .conversations
        .iter()
        .find(|c| c.id == fact.conversation)?;
    let at = conversation.sequence.iter().position(|id| *id == fact.id)?;
    let later: Vec<&str> = truth
        .calls
        .iter()
        .filter(|c| c.conversation == fact.conversation && c.succeeded())
        .filter(|c| {
            c.outputs
                .iter()
                .any(|o| conversation.sequence.iter().position(|id| id == o) > Some(at))
        })
        .map(|c| c.id.as_str())
        .collect();
    let recorded = truth.requests.get(fixture);
    for call in later {
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
            let haystack = Haystack::of_fixture(paths);
            for fact in authored {
                match refusal(&checked, fixture, fact, &haystack) {
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

/// The proof refuses a text no payload holds as written, one a response holds, and one a later request was
/// not sent.
#[test]
fn a_framework_text_is_refused_where_it_does_not_hold() {
    let fixture = "agentscope/sdk/structured_output";
    let paths = crate::discover_fixtures()
        .into_iter()
        .find(|(label, _)| label == fixture)
        .map(|(_, paths)| paths)
        .expect("the capture");
    let truth = super::truth::load_all()["agentscope/structured_output"].for_fixture(fixture);
    let haystack = Haystack::of_fixture(&paths);
    let authored = truth
        .facts
        .iter()
        .find(|f| f.evidence == "framework")
        .expect("a framework text")
        .clone();
    assert_eq!(refusal(&truth, fixture, &authored, &haystack), None);
    let mut altered = authored.clone();
    altered.value["text"] = "The required structured output is generated!".into();
    assert!(
        refusal(&truth, fixture, &altered, &haystack).is_some(),
        "a text no payload holds"
    );
    let mut answered = truth.clone();
    let output = answered
        .facts
        .iter_mut()
        .find(|f| f.call.is_some())
        .expect("a response part");
    output.kind = "text".into();
    output.value = serde_json::json!({"text": format!("Done. {}", authored.text())});
    assert!(
        refusal(&answered, fixture, &authored, &haystack).is_some(),
        "a text a response holds"
    );
    // A later call whose recorded request was not sent it.
    let mut later = truth.clone();
    let mut call = later.calls[0].clone();
    call.id = "call-later".into();
    call.outputs = vec!["fact-later".into()];
    let mut part = later
        .facts
        .iter()
        .find(|f| f.call.is_some())
        .expect("a response part")
        .clone();
    part.id = "fact-later".into();
    part.call = Some("call-later".into());
    later.conversations[0].sequence.push("fact-later".into());
    later.facts.push(part);
    later.calls.push(call);
    let request = later.requests.get_mut(fixture).expect("a recorded request");
    let sent = request.calls.values().next().expect("a request").clone();
    request.calls.insert("call-later".into(), sent);
    assert!(
        refusal(&later, fixture, &authored, &haystack).is_some(),
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
    let mut missing = recon.clone();
    remove_positions(&mut missing, in_trace);
    let new = added(&truth, &missing, &baseline);
    assert!(
        new.contains(&format!("text.missing:{}", authored.id)),
        "the text missing passed: {new:?}"
    );
}
