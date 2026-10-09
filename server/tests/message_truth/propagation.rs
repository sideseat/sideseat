//! A call whose trace context the producer lost (`trace_not_propagated`): the producer started a new trace for
//! it, so the history it was sent, and the results its request carried, are shown in that trace rather than in
//! the conversation's. Neither a leak nor a duplicate there - the same conversation, split by the telemetry.

use std::collections::{BTreeMap, BTreeSet};

use super::matching::Matching;
use super::predicates::{Shows, shows};
use super::recon::{Recon, ViewKind};
use super::truth::Truth;

/// By fact, the traces or spans it is also at home in.
pub(super) type Homes = BTreeMap<String, BTreeSet<String>>;

/// The calls the producer started a new trace for.
pub(super) fn untraced_calls(truth: &Truth) -> BTreeSet<&str> {
    truth
        .gaps
        .iter()
        .filter(|g| g.reason == "trace_not_propagated")
        .filter_map(|g| g.subject.as_deref())
        .collect()
}

/// For each fact an untraced call was sent - every fact its conversation holds before the call's own output -
/// the traces it is also at home in, and the spans it was sent on: those of the untraced calls.
pub(super) fn moved_homes(truth: &Truth, recon: &Recon, matching: &Matching) -> (Homes, Homes) {
    let mut homes = Homes::new();
    let mut spans = Homes::new();
    for call_id in untraced_calls(truth) {
        let Some(call) = truth.calls.iter().find(|c| c.id == call_id) else {
            continue;
        };
        let Some(&span) = matching.span_of.get(call_id) else {
            continue;
        };
        let (trace, span) = (
            &recon.generations[span].trace,
            &recon.generations[span].span,
        );
        let Some(conversation) = truth
            .conversations
            .iter()
            .find(|c| c.id == call.conversation)
        else {
            continue;
        };
        for fact in conversation
            .sequence
            .iter()
            .take_while(|id| !call.outputs.contains(id))
        {
            homes.entry(fact.clone()).or_default().insert(trace.clone());
            spans.entry(fact.clone()).or_default().insert(span.clone());
        }
    }
    (homes, spans)
}

/// The facts the session view cannot hold: those a trace view shows only in the moved traces, where those
/// traces belong to no session - the producer stated none on any of their spans.
pub(super) fn sessionless(truth: &Truth, recon: &Recon, moved: &Homes) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (fact_id, traces) in moved {
        let Some(fact) = truth.facts.iter().find(|f| &f.id == fact_id) else {
            continue;
        };
        let call_id = fact.value.get("call_id").and_then(|v| v.as_str());
        let shown_in: BTreeSet<&str> = recon
            .views
            .iter()
            .filter(|v| v.kind == ViewKind::Trace)
            .filter(|v| {
                v.blocks
                    .iter()
                    .any(|b| shows(fact, b, call_id) != Shows::No)
            })
            .map(|v| v.key.as_str())
            .collect();
        if !shown_in.is_empty()
            && shown_in
                .iter()
                .all(|t| traces.contains(*t) && !recon.session_of_trace.contains_key(*t))
        {
            out.insert(fact_id.clone());
        }
    }
    out
}

/// `trace_not_propagated` holds where the call's span has no parent and sits in a trace no earlier call of its
/// conversation is tied to, and - where those calls' traces belong to a session - no span of the call's trace
/// carries that session's id in any attribute: the producer started a trace for it and stated no session on it.
/// Anything else - a parent, an earlier call's trace, the session id, a first call, an untied call - is refused.
pub(super) fn prove_untraced(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
) -> super::absence::Proof {
    use super::absence::Proof;
    let Some(position) = truth.calls.iter().position(|c| c.id == call_id) else {
        return Proof::Unprovable(format!("no call {call_id}"));
    };
    let call = &truth.calls[position];
    let Some(&span) = matching.span_of.get(call_id) else {
        return Proof::Unprovable(format!("{call_id} is tied to no span"));
    };
    let generation = &recon.generations[span];
    let earlier: Vec<&str> = truth.calls[..position]
        .iter()
        .filter(|c| c.conversation == call.conversation && c.succeeded())
        .filter_map(|c| matching.span_of.get(&c.id))
        .map(|&g| recon.generations[g].trace.as_str())
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
    use crate::message_truth::absence::Proof;

    /// The call answering Spring AI's streamed tool round is in a trace of its own, with no parent and no
    /// session: proven, and what it was sent is at home there. A parent span, or an earlier call's trace,
    /// refuses it.
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
        let (homes, _) = moved_homes(&truth, &recon, &matching);
        let moved = &recon.generations[matching.span_of["call-002"]].trace;
        assert!(homes["fact-001"].contains(moved), "the prompt it was sent");
        let mut parented = recon.clone();
        let at = matching.span_of["call-002"];
        parented.generations[at].ancestors = vec!["a-parent".to_string()];
        assert!(matches!(
            prove_untraced(&truth, "call-002", &parented, &matching),
            Proof::Present(_)
        ));
        // The session search finds the id where a span states it - the conversation's own trace - and not in
        // the trace the producer started.
        let home = &recon.generations[matching.span_of["call-001"]].trace;
        let session = recon.session_of_trace[home].as_str();
        let sessions = BTreeSet::from([session]);
        assert!(session_stated(&recon.paths, home, &sessions).is_some());
        assert_eq!(session_stated(&recon.paths, moved, &sessions), None);
        let mut same = recon.clone();
        same.generations[at].trace = recon.generations[matching.span_of["call-001"]]
            .trace
            .clone();
        assert!(matches!(
            prove_untraced(&truth, "call-002", &same, &matching),
            Proof::Present(_)
        ));
    }
}
