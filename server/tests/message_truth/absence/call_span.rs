//! A model call recorded on no span of its own: the producer writes no model-call span at all, and the span
//! of the call before it - an agent span holding the whole run - carries this call's output too.

use std::collections::BTreeSet;

use super::{Haystack, Proof, prove};
use crate::message_truth::matching::{Matching, outputs, shared_span, shown_in, signature};
use crate::message_truth::recon::Recon;
use crate::message_truth::truth::Truth;

/// `call_span_not_exported` holds where no span of the capture is a model call - typed so by the reading, or
/// marked so in the raw payloads by a published convention; had the producer written one, the call could have
/// its own - and the call answers a tool round on the span it shares (`shared_span`).
/// That span shows every asserted output of the call. No other span but one enclosing it shows any of them as
/// its output, as the reconstruction reads them, and no raw carrier of a span open while the run was - in any
/// trace, the run's span and those enclosing it aside - holds them: a span of its own, typed or not, would
/// carry the response - a text, or a message holding a tool call's arguments. Anything else is refused: a model-call span, another span with the output, a call that answers no
/// tool round, a call with nothing asserted, or a shared span that does not carry all of the output.
pub(super) fn prove_call_span(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
    haystack: &Haystack,
) -> Proof {
    if let Some(generation) = recon.generations.iter().find(|g| g.typed) {
        return Proof::Present(format!("{} is a model-call span", generation.label));
    }
    if let Some(at) = raw_model_call_span(&recon.paths) {
        return Proof::Present(format!("{at} is a model-call span in the raw payloads"));
    }
    let Some(call) = truth.calls.iter().find(|c| c.id == call_id) else {
        return Proof::Unprovable(format!("no call {call_id}"));
    };
    let span = match shared_span(truth, matching, call_id) {
        Ok(span) => span,
        Err(why) => return Proof::Unprovable(why),
    };
    let shared = outputs(recon, &recon.generations[span]);
    let owed = signature(truth, call);
    if owed.is_empty() {
        return Proof::Unprovable(format!("{call_id} has no asserted output to look for"));
    }
    if let Some(fact) = owed.iter().find(|fact| !shown_in(fact, &shared)) {
        return Proof::Unprovable(format!(
            "{} does not show {}, an output of this call",
            recon.generations[span].label, fact.id
        ));
    }
    // A span enclosing the run's - a crew reporting its result - restates the run's output and is not one.
    let run: BTreeSet<&str> = std::iter::once(recon.generations[span].span.as_str())
        .chain(recon.generations[span].ancestors.iter().map(String::as_str))
        .collect();
    if let Some(own) = recon
        .generations
        .iter()
        .filter(|g| !run.contains(g.span.as_str()))
        .find(|g| {
            let shown = outputs(recon, g);
            owed.iter().any(|fact| shown_in(fact, &shown))
        })
    {
        return Proof::Present(format!("{} shows this call's output", own.label));
    }
    // In the raw carriers, where a span of its own would be: any span, in any trace, that was open while the run
    // was - the call happened then - other than the run's span and those enclosing it. Not a span that opened
    // after the run closed: the next agent of a crew is handed this call's answer as its task.
    let during = concurrent(&recon.paths, &recon.generations[span].span, &run);
    let elsewhere = Haystack {
        carriers: haystack
            .carriers
            .iter()
            .filter(|c| c.span.as_deref().is_some_and(|s| during.contains(s)))
            .cloned()
            .collect(),
        undecoded: haystack.undecoded.clone(),
    };
    if let Some(at) = elsewhere.undecoded.first() {
        return Proof::Unprovable(format!("{at} is encoded deeper than the search decodes"));
    }
    for fact in &owed {
        if fact.kind != "tool_call" {
            let proof = prove(fact, &elsewhere);
            if proof != Proof::Absent {
                return Proof::Present(format!("during the run, {}: {proof:?}", fact.id));
            }
            continue;
        }
        // The span that ran a call records its arguments; a message holding them - a role beside them - or a
        // call naming the tool beside them is a response, or a request re-sending one, which a model-call
        // span would carry, whatever shape it writes it in.
        let arguments = &fact.value["arguments"];
        if !super::identifying(arguments) {
            return Proof::Unprovable(format!(
                "{}'s arguments are too plain to tell a message holding them",
                fact.id
            ));
        }
        let name = fact.value.get("name").and_then(serde_json::Value::as_str);
        for carrier in &elsewhere.carriers {
            if let Some(at) = super::find_node(carrier, |node| {
                super::holds(node, arguments)
                    && (node.get("role").is_some_and(serde_json::Value::is_string)
                        || name.is_some_and(|name| names(node, name)))
            }) {
                return Proof::Present(format!(
                    "{at}, during the run, is a response holding the call {}",
                    fact.id
                ));
            }
        }
    }
    Proof::Absent
}

/// A raw span the published conventions mark as a model call, whatever the reading made of it: OpenInference's
/// `LLM` kind, a GenAI operation that calls a model, or a requested model stated.
fn raw_model_call_span(paths: &[std::path::PathBuf]) -> Option<String> {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    const MODEL_OPERATIONS: &[&str] = &["chat", "text_completion", "generate_content"];
    for path in paths {
        let request = crate::decode_request(path);
        for span in request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            let marked = span.attributes.iter().any(|kv| {
                let text = match kv.value.as_ref().and_then(|v| v.value.as_ref()) {
                    Some(Value::StringValue(s)) => s.as_str(),
                    _ => "",
                };
                matches!(kv.key.as_str(), "gen_ai.request.model" | "llm.model_name")
                    || (kv.key == "openinference.span.kind" && text == "LLM")
                    || (kv.key == "gen_ai.operation.name" && MODEL_OPERATIONS.contains(&text))
            });
            if marked {
                return Some(format!("span {:?}", span.name));
            }
        }
    }
    None
}

/// Whether an object names the tool in one of its members, directly or in a nested `function`: a call written
/// as `{name, arguments}` or `{function: {name, arguments}}`, with or without a role.
fn names(node: &serde_json::Value, name: &str) -> bool {
    let Some(object) = node.as_object() else {
        return false;
    };
    object.values().any(|v| v.as_str() == Some(name))
        || object.get("function").is_some_and(|f| names(f, name))
}

/// Every raw span, in any trace, open at some instant the run's span (`own`) was - its interval overlapping the
/// run's - other than the run's span and those enclosing it (`run`), as hex.
fn concurrent(paths: &[std::path::PathBuf], own: &str, run: &BTreeSet<&str>) -> BTreeSet<String> {
    let hex = crate::message_truth::truth::hex_digest;
    let mut times: Vec<(String, u64, u64)> = Vec::new();
    for path in paths {
        let request = crate::decode_request(path);
        for span in request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            times.push((
                hex(&span.span_id),
                span.start_time_unix_nano,
                span.end_time_unix_nano,
            ));
        }
    }
    let Some(&(_, run_start, run_end)) = times.iter().find(|(id, _, _)| id == own) else {
        return BTreeSet::new();
    };
    times
        .into_iter()
        .filter(|(id, start, end)| {
            !run.contains(id.as_str()) && *start < run_end && *end > run_start
        })
        .map(|(id, _, _)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a capture with no model-call span the call answering a tool round shares the run's span, and that is
    /// proven; a model-call span anywhere, another span showing the output in the reading or in a raw carrier,
    /// a first call, a call answering no tool round, or a shared span missing one output refuses it.
    #[test]
    fn a_call_shares_the_run_s_span_only_where_no_model_call_span_exists() {
        let fixture = "crewai/sdk/tool_use";
        let paths = crate::discover_fixtures()
            .into_iter()
            .find(|(label, _)| label == fixture)
            .map(|(_, paths)| paths)
            .expect("the capture");
        let truths = crate::message_truth::truth::load_all();
        let truth = truths["crewai/tool_use"].for_fixture(fixture);
        let recon = crate::message_truth::recon::build(fixture, &paths);
        let matching = crate::message_truth::matching::match_calls(&truth, &recon, &mut Vec::new());
        let haystack = Haystack::of_fixture(&paths);
        let proof = |truth: &Truth, call: &str, recon: &Recon, haystack: &Haystack| {
            prove_call_span(truth, call, recon, &matching, haystack)
        };
        assert_eq!(proof(&truth, "call-002", &recon, &haystack), Proof::Absent);
        assert!(matches!(
            proof(&truth, "call-001", &recon, &haystack),
            Proof::Unprovable(_)
        ));
        // A call after one that asked for no tool answers no tool round.
        let mut plain = truth.clone();
        for fact in plain
            .facts
            .iter_mut()
            .filter(|f| f.call.as_deref() == Some("call-001"))
        {
            fact.kind = "text".into();
        }
        assert!(matches!(
            proof(&plain, "call-002", &recon, &haystack),
            Proof::Unprovable(_)
        ));
        // A raw carrier open during the run holding the answer: a span of its own the reading missed.
        let answer = *signature(&truth, &truth.calls[1])
            .last()
            .expect("an output");
        let shared_span = &recon.generations[matching.span_of["call-001"]].span;
        let outside = recon
            .generations
            .iter()
            .find(|g| g.ancestors.first() == Some(shared_span))
            .map(|g| g.span.clone())
            .expect("a span within the run");
        let mut missed = Haystack {
            carriers: haystack.carriers.clone(),
            undecoded: Vec::new(),
        };
        missed.carriers.push(super::super::Carrier {
            span: Some(outside),
            strings: vec![(
                "a missed span".to_string(),
                super::super::haystack::collapse_whitespace(answer.text()),
            )],
            ..Default::default()
        });
        assert!(matches!(
            proof(&truth, "call-002", &recon, &missed),
            Proof::Present(_)
        ));
        // The run's span without one of the call's outputs: the call is then missing, not sharing it.
        let mut cut = recon.clone();
        let shared = cut.generations[matching.span_of["call-001"]].span.clone();
        let last = *signature(&truth, &truth.calls[1])
            .last()
            .expect("an output");
        for view in cut
            .views
            .iter_mut()
            .filter(|v| v.kind == crate::message_truth::recon::ViewKind::Span && v.key == shared)
        {
            view.blocks.retain(|b| !(b.output && shown_in(last, &[b])));
        }
        assert!(matches!(
            proof(&truth, "call-002", &cut, &haystack),
            Proof::Unprovable(_)
        ));
        // Another span showing the call's output is a span of its own, typed or not; one enclosing the run's
        // span restating it is not.
        let mut own = recon.clone();
        let enclosing = own.generations[matching.span_of["call-001"]]
            .ancestors
            .clone();
        let other = own
            .generations
            .iter()
            .find(|g| g.span != shared && !enclosing.contains(&g.span))
            .map(|g| g.span.clone())
            .expect("a second span");
        let copy = own
            .span_view(&shared)
            .and_then(|v| v.blocks.iter().find(|b| b.output && shown_in(last, &[b])))
            .cloned()
            .expect("the shared span shows the output");
        for view in own
            .views
            .iter_mut()
            .filter(|v| v.kind == crate::message_truth::recon::ViewKind::Span && v.key == other)
        {
            view.blocks.push(copy.clone());
        }
        assert!(matches!(
            proof(&truth, "call-002", &own, &haystack),
            Proof::Present(_)
        ));
        // The raw search finds a model-call span where a convention marks one - Spring AI's OpenInference
        // `LLM` spans - and none in CrewAI's capture.
        let spring = crate::discover_fixtures()
            .into_iter()
            .find(|(label, _)| label == "spring-ai/sdk/streaming")
            .map(|(_, paths)| paths)
            .expect("a capture with model-call spans");
        assert!(raw_model_call_span(&spring).is_some());
        assert_eq!(raw_model_call_span(&paths), None);
        let mut typed = recon.clone();
        typed.generations[0].typed = true;
        assert!(matches!(
            proof(&truth, "call-002", &typed, &haystack),
            Proof::Present(_)
        ));
    }
}
