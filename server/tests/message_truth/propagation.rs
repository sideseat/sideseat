//! A call whose trace context the producer lost (`trace_not_propagated`): the producer started a new trace for
//! it, and that trace's view shows what the call was sent beside what it answered. It is checked as a view of
//! its own (`checks::scopes`), owing each fact it was sent - every one a raw carrier of that trace holds -
//! exactly once, in order, on the call's span; the conversation's views leave that trace out.

use std::collections::{BTreeMap, BTreeSet};

use super::absence::haystack::Haystack;
use super::absence::{Proof, prove};
use super::matching::Matching;
use super::recon::Recon;
use super::truth::{Fact, Truth};

/// One trace the producer started for a call.
#[derive(Debug, Clone)]
pub(super) struct SplitTrace {
    pub call: String,
    pub trace: String,
    /// The call's span: where everything it was sent is shown in its trace.
    pub span: String,
    /// The capture-stable label of the trace, which names this view's obligations (`fact@trace-2`).
    pub label: String,
    /// The facts the call was sent that a raw carrier of the trace holds.
    pub history: BTreeSet<String>,
}

/// The traces the producer split a conversation into, and the facts that are in no other trace's raw
/// carriers, which the conversation's own views cannot show.
#[derive(Debug, Default)]
pub(super) struct Splits {
    pub traces: Vec<SplitTrace>,
    pub only_there: BTreeSet<String>,
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

/// The split-off traces of a capture, read from its raw payloads: what each was sent is what a raw carrier of
/// that trace holds, and a fact no carrier outside the split-off traces holds is theirs alone.
pub(super) fn split(truth: &Truth, recon: &Recon, matching: &Matching) -> Splits {
    let mut out = Splits::default();
    let untraced = untraced_calls(truth);
    if untraced.is_empty() {
        return out;
    }
    let haystack = Haystack::of_fixture(&recon.paths);
    let trace_of = span_traces(&recon.paths);
    for call_id in untraced {
        let Some(call) = truth
            .calls
            .iter()
            .find(|c| c.id == call_id && c.succeeded())
        else {
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
        let Some(end) = conversation
            .sequence
            .iter()
            .position(|id| call.outputs.contains(id))
        else {
            continue;
        };
        let history = conversation.sequence[..end]
            .iter()
            .filter_map(|id| truth.facts.iter().find(|f| &f.id == id))
            .filter(|f| holds(&haystack, &trace_of, f, |t| t == generation.trace))
            .map(|f| f.id.clone())
            .collect();
        out.traces.push(SplitTrace {
            call: call_id.to_string(),
            trace: generation.trace.clone(),
            span: generation.span.clone(),
            label: generation
                .label
                .split_once('/')
                .map_or_else(|| generation.trace.clone(), |(label, _)| label.to_string()),
            history,
        });
    }
    let split: BTreeSet<&str> = out.traces.iter().map(|t| t.trace.as_str()).collect();
    for trace in &out.traces {
        for id in &trace.history {
            let Some(fact) = truth.facts.iter().find(|f| &f.id == id) else {
                continue;
            };
            if !holds(&haystack, &trace_of, fact, |t| !split.contains(t)) {
                out.only_there.insert(id.clone());
            }
        }
    }
    out
}

/// Whether a raw carrier of a span in the traces `keep` admits holds the fact: anything but a proven absence,
/// so a fact the search cannot rule out counts as held.
fn holds(
    haystack: &Haystack,
    trace_of: &BTreeMap<String, String>,
    fact: &Fact,
    keep: impl Fn(&str) -> bool,
) -> bool {
    let within = Haystack {
        carriers: haystack
            .carriers
            .iter()
            .filter(|c| {
                c.span
                    .as_deref()
                    .and_then(|s| trace_of.get(s))
                    .is_some_and(|t| keep(t))
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

/// `trace_not_propagated` holds where a successful call answers a tool round - the call right before it in its
/// conversation asked for a tool - and its span has no parent in the raw payloads, sits in a trace no earlier
/// call of its conversation is tied to, and belongs to no session: the reconstruction joins it to none, and no
/// raw span or resource of its trace states one (`session_stated`). Anything else - a failed call, one
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
    if !call.succeeded() || call.outputs.is_empty() {
        return Proof::Unprovable(format!("{call_id} produced no output"));
    }
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
    match raw_parent(&recon.paths, &generation.span) {
        None => return Proof::Unprovable(format!("{} is in no raw payload", generation.label)),
        Some(Some(parent)) => {
            return Proof::Present(format!("{} has a parent span, {parent}", generation.label));
        }
        Some(None) => {}
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
        return Proof::Present(format!("{at} states a session"));
    }
    Proof::Absent
}

/// A raw span's parent, as hex: `None` when no payload holds the span, `Some(None)` for a root.
fn raw_parent(paths: &[std::path::PathBuf], span: &str) -> Option<Option<String>> {
    for path in paths {
        let request = crate::decode_request(path);
        if let Some(found) = request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
            .find(|s| super::truth::hex_digest(&s.span_id) == span)
        {
            return Some(
                (!found.parent_span_id.is_empty())
                    .then(|| super::truth::hex_digest(&found.parent_span_id)),
            );
        }
    }
    None
}

/// The attribute keys the published conventions state a session or conversation under.
const SESSION_KEYS: &[&str] = &["session.id", "gen_ai.conversation.id"];

/// Where a raw span of the trace, its events, or the resource it was exported under states a session: a key
/// the conventions name one under (or any key ending in `.session.id` or `.session_id`), or one of the given
/// session ids as an attribute's whole value - the ways a producer could have joined the trace to one.
fn session_stated(
    paths: &[std::path::PathBuf],
    trace: &str,
    sessions: &BTreeSet<&str>,
) -> Option<String> {
    use opentelemetry_proto::tonic::common::v1::{KeyValue, any_value::Value};
    let carries = |attributes: &[KeyValue]| {
        attributes.iter().find_map(|kv| {
            let named = SESSION_KEYS.contains(&kv.key.as_str())
                || kv.key.ends_with(".session.id")
                || kv.key.ends_with(".session_id");
            let valued = matches!(
                kv.value.as_ref().and_then(|v| v.value.as_ref()),
                Some(Value::StringValue(s)) if sessions.contains(s.as_str())
            );
            (named || valued).then(|| kv.key.clone())
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
    /// session in the raw payloads: proven, and its trace's view is owed what that trace's raw carriers hold
    /// of the conversation. A failed call, one answering no tool round, a raw parent, a session, or an earlier
    /// call's trace refuses it.
    #[test]
    fn a_call_in_a_trace_of_its_own_is_checked_in_that_trace() {
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
        // The raw parents: the first call's span is a child of the run's root, the split-off call's a root.
        assert!(matches!(
            raw_parent(&recon.paths, &first.span),
            Some(Some(_))
        ));
        assert_eq!(
            raw_parent(&recon.paths, &recon.generations[at].span),
            Some(None)
        );
        let splits = split(&truth, &recon, &matching);
        let [only] = splits.traces.as_slice() else {
            panic!("one split-off trace: {splits:?}");
        };
        assert_eq!(&only.trace, moved);
        // The prompt and the first result are in that trace's carriers; the result in no other trace's.
        assert!(only.history.contains("fact-001") && only.history.contains("fact-005"));
        assert!(splits.only_there.contains("fact-005"));
        assert!(!splits.only_there.contains("fact-001"));
        // A failed call, and one after a call that asked for no tool, are refused.
        let mut failed = truth.clone();
        failed.calls[1].outcome = "failed".into();
        assert!(matches!(
            prove_untraced(&failed, "call-002", &recon, &matching),
            Proof::Unprovable(_)
        ));
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
        let mut joined = recon.clone();
        joined
            .session_of_trace
            .insert(moved.clone(), "another-session".into());
        assert!(matches!(
            prove_untraced(&truth, "call-002", &joined, &matching),
            Proof::Present(_)
        ));
        // The session search finds a session where a span states one - the conversation's own trace - and not
        // in the trace the producer started.
        let session = recon.session_of_trace[home].as_str();
        assert!(session_stated(&recon.paths, home, &BTreeSet::new()).is_some());
        assert!(session_stated(&recon.paths, home, &BTreeSet::from([session])).is_some());
        assert_eq!(
            session_stated(&recon.paths, moved, &BTreeSet::from([session])),
            None
        );
        let mut same = recon.clone();
        same.generations[at].trace = home.clone();
        assert!(matches!(
            prove_untraced(&truth, "call-002", &same, &matching),
            Proof::Present(_)
        ));
    }
}
