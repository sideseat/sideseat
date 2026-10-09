//! A call whose trace context the producer lost (`trace_not_propagated`): the producer started a new trace for
//! it, so the history it was sent, and the results its request carried, are shown in that trace as well as in
//! the conversation's. Neither a leak nor a duplicate there - the same conversation, split by the telemetry - so
//! that trace is one more home of each fact it was sent, owed one copy like every home (`Context::home_traces`).

use std::collections::{BTreeMap, BTreeSet};

use super::absence::haystack::Haystack;
use super::absence::{Proof, prove};
use super::matching::Matching;
use super::recon::Recon;
use super::truth::{Fact, Truth};

/// What a split-off trace adds to each fact it was sent: its homes, the spans it was sent on, and whether a
/// session can hold it at all.
#[derive(Debug, Default)]
pub(super) struct Split {
    /// By fact, the traces it is at home in: each split-off trace it was sent in, and its own home trace where
    /// a raw carrier of that trace holds it.
    pub homes: BTreeMap<String, BTreeSet<String>>,
    /// By fact, the spans it is shown on: the split-off calls' spans, and its home call's span with its trace.
    pub spans: BTreeMap<String, BTreeSet<String>>,
    /// The facts none of whose homes belongs to a session, so the session view cannot hold them.
    pub sessionless: BTreeSet<String>,
}

/// The calls the producer started a new trace for.
pub(super) fn untraced_calls(truth: &Truth) -> BTreeSet<&str> {
    truth
        .gaps
        .iter()
        .filter(|g| g.reason == "trace_not_propagated")
        .filter_map(|g| g.subject.as_deref())
        .collect()
}

/// The homes a split-off trace adds. `home` names a fact's ordinary home - its call's trace and span - as the
/// checks place it. That home stays one only where a raw carrier of its trace holds the fact: a result the
/// producer recorded only in the split-off call's request is in no carrier of the conversation's trace, and the
/// raw payloads, not the reconstruction under test, say so.
pub(super) fn split(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
    home: impl Fn(&Fact) -> Option<(String, String)>,
) -> Split {
    let mut out = Split::default();
    let untraced = untraced_calls(truth);
    if untraced.is_empty() {
        return out;
    }
    let haystack = Haystack::of_fixture(&recon.paths);
    let trace_of = span_traces(&recon.paths);
    for call_id in untraced {
        let Some(call) = truth.calls.iter().find(|c| c.id == call_id) else {
            continue;
        };
        let Some(&span) = matching.span_of.get(call_id) else {
            continue;
        };
        let generation = &recon.generations[span];
        let Some(conversation) = truth
            .conversations
            .iter()
            .find(|c| c.id == call.conversation)
        else {
            continue;
        };
        for id in conversation
            .sequence
            .iter()
            .take_while(|id| !call.outputs.contains(id))
        {
            out.homes
                .entry(id.clone())
                .or_default()
                .insert(generation.trace.clone());
            out.spans
                .entry(id.clone())
                .or_default()
                .insert(generation.span.clone());
        }
    }
    for (id, homes) in out.homes.iter_mut() {
        let Some(fact) = truth.facts.iter().find(|f| &f.id == id) else {
            continue;
        };
        if let Some((trace, span)) = home(fact)
            && trace_holds(&haystack, &trace_of, &trace, fact)
        {
            homes.insert(trace);
            out.spans.entry(id.clone()).or_default().insert(span);
        }
        if !homes.iter().any(|t| recon.session_of_trace.contains_key(t)) {
            out.sessionless.insert(id.clone());
        }
    }
    out
}

/// Whether a raw carrier of the trace holds the fact: anything but a proven absence, so a fact the search
/// cannot rule out stays owed there.
fn trace_holds(
    haystack: &Haystack,
    trace_of: &BTreeMap<String, String>,
    trace: &str,
    fact: &Fact,
) -> bool {
    let within = Haystack {
        carriers: haystack
            .carriers
            .iter()
            .filter(|c| {
                c.span
                    .as_deref()
                    .and_then(|s| trace_of.get(s))
                    .is_some_and(|t| t == trace)
            })
            .cloned()
            .collect(),
        undecoded: haystack.undecoded.clone(),
    };
    prove(fact, &within) != Proof::Absent
}

/// Each raw span's trace, both as hex.
fn span_traces(paths: &[std::path::PathBuf]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for path in paths {
        let request = crate::decode_request(path);
        for span in request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            out.insert(
                super::truth::hex_digest(&span.span_id),
                super::truth::hex_digest(&span.trace_id),
            );
        }
    }
    out
}

/// `trace_not_propagated` holds where the call answers a tool round - the call right before it in its
/// conversation asked for a tool - and its span has no parent, sits in a trace no earlier call of its
/// conversation is tied to, and belongs to no session: the reconstruction joins it to none, and no span of its
/// trace carries the session id of the conversation's earlier traces in any attribute. Anything else - a call
/// answering no tool round, a parent, an earlier call's trace, a session, a first call, an untied call - is
/// refused.
pub(super) fn prove_untraced(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
) -> Proof {
    let Some(position) = truth.calls.iter().position(|c| c.id == call_id) else {
        return Proof::Unprovable(format!("no call {call_id}"));
    };
    let call = &truth.calls[position];
    let Some(&span) = matching.span_of.get(call_id) else {
        return Proof::Unprovable(format!("{call_id} is tied to no span"));
    };
    let generation = &recon.generations[span];
    let before: Vec<_> = truth.calls[..position]
        .iter()
        .filter(|c| c.conversation == call.conversation && c.succeeded())
        .collect();
    let Some(previous) = before.last() else {
        return Proof::Unprovable(
            "the first call of its conversation starts its trace".to_string(),
        );
    };
    let asked = previous.outputs.iter().any(|id| {
        truth
            .facts
            .iter()
            .any(|f| &f.id == id && f.kind == "tool_call")
    });
    if !asked {
        return Proof::Unprovable(format!("{} asked for no tool", previous.id));
    }
    let earlier: Vec<&str> = before
        .iter()
        .filter_map(|c| matching.span_showing(&c.id))
        .map(|g| recon.generations[g].trace.as_str())
        .collect();
    if earlier.is_empty() {
        return Proof::Unprovable(
            "no earlier call of its conversation is tied to a span".to_string(),
        );
    }
    if let Some(parent) = generation.ancestors.first() {
        return Proof::Present(format!("{} has a parent span, {parent}", generation.label));
    }
    if earlier.contains(&generation.trace.as_str()) {
        return Proof::Present(format!(
            "{} is in the trace of an earlier call of its conversation",
            generation.label
        ));
    }
    if let Some(session) = recon.session_of_trace.get(&generation.trace) {
        return Proof::Present(format!("{} belongs to session {session}", generation.label));
    }
    let sessions: BTreeSet<&str> = earlier
        .iter()
        .filter_map(|t| recon.session_of_trace.get(*t))
        .map(String::as_str)
        .collect();
    if let Some(at) = session_stated(&recon.paths, &generation.trace, &sessions) {
        return Proof::Present(format!("{at} states the conversation's session"));
    }
    Proof::Absent
}

/// Where a span of the trace, or the resource it was exported under, carries one of the session ids as an
/// attribute's whole value - the one way a producer could have joined the trace to that session.
fn session_stated(
    paths: &[std::path::PathBuf],
    trace: &str,
    sessions: &BTreeSet<&str>,
) -> Option<String> {
    use opentelemetry_proto::tonic::common::v1::{KeyValue, any_value::Value};
    if sessions.is_empty() {
        return None;
    }
    let carries = |attributes: &[KeyValue]| {
        attributes
            .iter()
            .find_map(|kv| match kv.value.as_ref()?.value.as_ref()? {
                Value::StringValue(s) if sessions.contains(s.as_str()) => Some(kv.key.clone()),
                _ => None,
            })
    };
    for path in paths {
        let request = crate::decode_request(path);
        for resource_spans in &request.resource_spans {
            for span in resource_spans
                .scope_spans
                .iter()
                .flat_map(|s| &s.spans)
                .filter(|s| super::truth::hex_digest(&s.trace_id) == trace)
            {
                let resource = resource_spans
                    .resource
                    .as_ref()
                    .map(|r| r.attributes.as_slice());
                let found = carries(&span.attributes)
                    .or_else(|| resource.and_then(carries))
                    .or_else(|| span.events.iter().find_map(|e| carries(&e.attributes)));
                if let Some(key) = found {
                    return Some(format!(
                        "span {} ({key})",
                        super::truth::hex_digest(&span.span_id)
                    ));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The call answering Spring AI's streamed tool round is in a trace of its own, with no parent and no
    /// session: proven, and what it was sent is at home there - and in the conversation's trace only where a
    /// raw carrier of that trace holds it. A call answering no tool round, a parent span, a session, or an
    /// earlier call's trace refuses it.
    #[test]
    fn a_call_in_a_trace_of_its_own_moves_what_it_was_sent() {
        let fixture = "spring-ai/sdk/streaming";
        let paths = crate::discover_fixtures()
            .into_iter()
            .find(|(label, _)| label == fixture)
            .map(|(_, paths)| paths)
            .expect("the capture");
        let truths = super::super::truth::load_all();
        let truth = truths["spring-ai/streaming"].for_fixture(fixture);
        let recon = super::super::recon::build(fixture, &paths);
        let matching = super::super::matching::match_calls(&truth, &recon, &mut Vec::new());
        assert_eq!(
            prove_untraced(&truth, "call-002", &recon, &matching),
            Proof::Absent
        );
        assert!(matches!(
            prove_untraced(&truth, "call-001", &recon, &matching),
            Proof::Unprovable(_)
        ));
        let at = matching.span_of["call-002"];
        let first = &recon.generations[matching.span_of["call-001"]];
        let (home, moved) = (&first.trace, &recon.generations[at].trace);
        let split = split(&truth, &recon, &matching, |_| {
            Some((first.trace.clone(), first.span.clone()))
        });
        // The prompt is in both traces' carriers; the first result only in the split-off call's request.
        assert_eq!(
            split.homes["fact-001"],
            BTreeSet::from([home.clone(), moved.clone()])
        );
        assert_eq!(split.homes["fact-005"], BTreeSet::from([moved.clone()]));
        assert!(split.sessionless.contains("fact-005"));
        assert!(!split.sessionless.contains("fact-001"));
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
            prove_untraced(&plain, "call-002", &recon, &matching),
            Proof::Unprovable(_)
        ));
        let mut parented = recon.clone();
        parented.generations[at].ancestors = vec!["a-parent".to_string()];
        assert!(matches!(
            prove_untraced(&truth, "call-002", &parented, &matching),
            Proof::Present(_)
        ));
        let mut joined = recon.clone();
        joined
            .session_of_trace
            .insert(moved.clone(), "another-session".into());
        assert!(matches!(
            prove_untraced(&truth, "call-002", &joined, &matching),
            Proof::Present(_)
        ));
        // The session search finds the id where a span states it - the conversation's own trace - and not in
        // the trace the producer started.
        let session = recon.session_of_trace[home].as_str();
        let sessions = BTreeSet::from([session]);
        assert!(session_stated(&recon.paths, home, &sessions).is_some());
        assert_eq!(session_stated(&recon.paths, moved, &sessions), None);
        let mut same = recon.clone();
        same.generations[at].trace = home.clone();
        assert!(matches!(
            prove_untraced(&truth, "call-002", &same, &matching),
            Proof::Present(_)
        ));
    }
}
