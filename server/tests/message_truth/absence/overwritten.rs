//! A model call whose own span the tool it led to wrote over (`call_span_overwritten_by_tool`): the producer traces
//! the tool on the current span, which is the call's step, so the step states the tool's execution - the
//! conventions' `execute_tool`, the tool's name and arguments - with its own request and response emptied. No span
//! can then be tied to the call by its output; the call's parts are asserted where the conversation shows them.
//!
//! ADK for Java 0.9.0 is the producer this was found on (`com.google.adk.telemetry.Tracing`: `traceToolCall` writes
//! through `traceWithSpan` on `Span.current()`, and `setToolExecutionAttributes` sets the operation and the two
//! empty objects). The test names the attributes that producer writes, as rubric code may.

use std::collections::{BTreeMap, BTreeSet};

use opentelemetry_proto::tonic::common::v1::any_value::Value as Otlp;
use serde_json::Value;

use super::Proof;
use super::haystack::{Carrier, Haystack};
use crate::message_truth::matching::Matching;
use crate::message_truth::recon::{Recon, ViewKind};
use crate::message_truth::truth::{Truth, hex_digest};

/// The step's own request and response, which the tool's tracer empties.
const REQUEST: &str = "gcp.vertex.agent.llm_request";
const RESPONSE: &str = "gcp.vertex.agent.llm_response";
/// The tool's arguments, as the tool's tracer writes them on the span.
const ARGUMENTS: &str = "gcp.vertex.agent.tool_call_args";

/// Everything a step the tool wrote over may carry: the step's own identity (the system, the model asked, the
/// invocation, event and session it belongs to) and the tool's execution (operation, tool name, description, type,
/// arguments, the emptied request and response). Anything else - another attribute, an event, a log record - could
/// hold the model's output, so it refuses the proof whatever it says.
const STEP_CARRIES: &[&str] = &[
    "gen_ai.system",
    "gen_ai.request.model",
    "gcp.vertex.agent.invocation_id",
    "gcp.vertex.agent.event_id",
    "gcp.vertex.agent.session_id",
    "gen_ai.operation.name",
    "gen_ai.tool.name",
    "gen_ai.tool.description",
    "gen_ai.tool.type",
    ARGUMENTS,
    REQUEST,
    RESPONSE,
];

/// One raw span, as the proof reads it: where it is, its name, and its string attributes.
#[derive(Debug, Clone)]
pub(super) struct Stated {
    pub trace: String,
    pub span: String,
    pub parent: String,
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    /// How many events the span holds: a step the tool wrote over holds none.
    pub events: usize,
    /// Whether any attribute key is written twice: the map keeps one value, so a second is refused rather than read.
    pub repeated: bool,
    /// How many links it holds, and its status message: carriers beyond the tool's execution too.
    pub links: usize,
    pub status: String,
}

fn stated_spans(recon: &Recon) -> Vec<Stated> {
    let mut out = Vec::new();
    for path in &recon.paths {
        let request = crate::decode_request(path);
        for span in request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            let attributes = span
                .attributes
                .iter()
                // Every attribute, a non-string one by its kind alone: the allowlist below reads the keys.
                .map(|kv| {
                    let value = match kv.value.as_ref().and_then(|v| v.value.as_ref()) {
                        Some(Otlp::StringValue(s)) => s.clone(),
                        _ => "<not a string>".to_string(),
                    };
                    (kv.key.clone(), value)
                })
                .collect();
            let keys: BTreeSet<&str> = span.attributes.iter().map(|kv| kv.key.as_str()).collect();
            let repeated = keys.len() != span.attributes.len();
            out.push(Stated {
                repeated,
                trace: hex_digest(&span.trace_id),
                span: hex_digest(&span.span_id),
                parent: hex_digest(&span.parent_span_id),
                name: span.name.clone(),
                attributes,
                events: span.events.len(),
                links: span.links.len(),
                status: span
                    .status
                    .as_ref()
                    .map(|s| s.message.clone())
                    .unwrap_or_default(),
            });
        }
    }
    out
}

/// Why the span does not state the overwrite of the call making `tool` with `arguments`, if it does not: it is
/// named as the producer names its model-call steps, states the tool's execution under the conventions'
/// operation and the call's tool name, holds the call's arguments, and holds an empty request and response.
pub(super) fn shape_refusal(
    span: &Stated,
    tool: &str,
    arguments: &Value,
    step_names: &BTreeSet<String>,
) -> Option<String> {
    let attribute = |key: &str| span.attributes.get(key).map(String::as_str);
    if !step_names.contains(&span.name) {
        return Some(format!(
            "{} is not named as the producer's model-call steps are",
            span.name
        ));
    }
    if attribute("gen_ai.operation.name") != Some("execute_tool") {
        return Some(format!("{} states no tool execution", span.name));
    }
    if attribute("gen_ai.tool.name") != Some(tool) {
        return Some(format!("{} names another tool than {tool}", span.name));
    }
    if attribute(ARGUMENTS)
        .and_then(|a| serde_json::from_str::<Value>(a).ok())
        .as_ref()
        != Some(arguments)
    {
        return Some(format!(
            "{} holds other arguments than the call's",
            span.name
        ));
    }
    if attribute(REQUEST) != Some("{}") || attribute(RESPONSE) != Some("{}") {
        return Some(format!(
            "{}'s own request and response are not both emptied",
            span.name
        ));
    }
    None
}

/// What the step carries beyond its own identity and the tool's execution (`STEP_CARRIES`), if anything: another
/// attribute, or an event.
pub(super) fn beyond_the_step(span: &Stated) -> Option<String> {
    if let Some(other) = span
        .attributes
        .keys()
        .find(|key| !STEP_CARRIES.contains(&key.as_str()))
    {
        return Some(format!(
            "{} carries {other}, beyond the tool's execution",
            span.name
        ));
    }
    if span.repeated {
        return Some(format!("{} writes an attribute twice", span.name));
    }
    (span.events > 0 || span.links > 0 || !span.status.is_empty()).then(|| {
        format!(
            "{} holds events, links or a status message, beyond the tool's execution",
            span.name
        )
    })
}

/// A call no span can be tied to, by the reason its gap gives: a silent request span, or one the tool overwrote.
pub(super) fn prove_spanless(
    reason: &str,
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
    haystack: &Haystack,
) -> Proof {
    if reason == "call_span_overwritten_by_tool" {
        prove_overwritten(truth, call_id, recon, matching, haystack)
    } else {
        super::request_span::prove_request_span_silent(truth, call_id, recon, haystack)
    }
}

/// `call_span_overwritten_by_tool` holds where the call succeeded with exactly one output, a tool call, and exactly
/// one span of the capture states the overwrite of that call (`shape_refusal`) while recording no model output:
/// no output block in its view, and neither the call's wire id nor a published response identifier in any of its
/// raw carriers. A call another span is tied to by its output refuses it, as does a capture where two spans could
/// be the call's.
pub(super) fn prove_overwritten(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
    haystack: &Haystack,
) -> Proof {
    let Some(call) = truth.calls.iter().find(|c| c.id == call_id) else {
        return Proof::Unprovable(format!("no call {call_id}"));
    };
    if matching.span_of.contains_key(call_id) {
        return Proof::Present(format!("{call_id} is tied to a span by its output"));
    }
    // The matcher leaves a call declaring this gap untied, so the call's own response is looked for apart from it:
    // a span stating the response's id, or a model-call span no call is tied to, could be the call's own span.
    if let Some(why) = identified_elsewhere(call, recon, matching, haystack) {
        return Proof::Present(why);
    }
    let outputs: Vec<_> = call
        .outputs
        .iter()
        .filter_map(|id| truth.facts.iter().find(|f| f.id == *id))
        // An output already withdrawn - reasoning no payload holds - is owed nowhere.
        .filter(|f| f.require.is_some())
        .collect();
    let [fact] = outputs.as_slice() else {
        return Proof::Unprovable(format!("{call_id} has not exactly one asserted output"));
    };
    let (Some(tool), Some(arguments)) = (
        fact.value.get("name").and_then(Value::as_str),
        fact.value.get("arguments"),
    ) else {
        return Proof::Unprovable(format!("{call_id}'s output is no tool call"));
    };
    let wire_id = super::wire_id(fact);
    // The producer's model-call steps: the spans tied to the conversation's other calls - their name, and where
    // they stand, by trace and parent. A step of the same run stands where they do.
    let all = stated_spans(recon);
    let steps: Vec<&Stated> = matching
        .span_of
        .iter()
        .filter(|(other, _)| {
            truth
                .calls
                .iter()
                .any(|c| c.id == **other && c.conversation == call.conversation)
        })
        .map(|(_, g)| g)
        .filter_map(|&g| {
            let generation = &recon.generations[g];
            all.iter()
                .find(|s| s.trace == generation.trace && s.span == generation.span)
        })
        .collect();
    let step_names: BTreeSet<String> = steps.iter().map(|s| s.name.clone()).collect();
    // One span may stand for one call only: two declared calls of one tool with the same arguments could each be
    // either span.
    let twins = truth
        .gaps
        .iter()
        .filter(|g| {
            g.reason == "call_span_overwritten_by_tool" && g.subject.as_deref() != Some(call_id)
        })
        .filter_map(|g| {
            truth
                .calls
                .iter()
                .find(|c| Some(c.id.as_str()) == g.subject.as_deref())
        })
        .flat_map(|c| &c.outputs)
        .filter_map(|id| truth.facts.iter().find(|f| f.id == *id))
        .any(|f| {
            f.value.get("name").and_then(Value::as_str) == Some(tool)
                && f.value.get("arguments") == Some(arguments)
        });
    if twins {
        return Proof::Unprovable(format!("another declared call makes {call_id}'s call too"));
    }
    // Every receipt of a candidate is judged, not one of them: a span delivered twice is one span only where each
    // delivery states the overwrite and carries nothing beyond it.
    let shaped = |span: &Stated| {
        shape_refusal(span, tool, arguments, &step_names).is_none()
            && steps
                .iter()
                .any(|step| step.trace == span.trace && step.parent == span.parent)
    };
    let mut identities: Vec<(&str, &str)> = all
        .iter()
        .filter(|span| shaped(span))
        .map(|span| (span.trace.as_str(), span.span.as_str()))
        .collect();
    identities.sort_unstable();
    identities.dedup();
    let (trace, id) = match identities.as_slice() {
        [one] => *one,
        [] => return Proof::Unprovable(format!("no span states the overwrite of {call_id}")),
        _ => return Proof::Unprovable(format!("several spans could be {call_id}'s")),
    };
    let receipts: Vec<&Stated> = all
        .iter()
        .filter(|s| s.trace == trace && s.span == id)
        .collect();
    for receipt in &receipts {
        if !shaped(receipt) {
            return Proof::Present(format!(
                "a delivery of {} does not state the overwrite",
                receipt.name
            ));
        }
        if let Some(why) = beyond_the_step(receipt) {
            return Proof::Present(why);
        }
    }
    let span = receipts[0];
    if haystack.carriers.iter().any(|c| {
        c.record
            && c.span.as_deref() == Some(span.span.as_str())
            && c.trace.as_deref().is_none_or(|t| t == span.trace)
    }) {
        return Proof::Present(format!(
            "{} emitted log records, beyond the tool's execution",
            span.name
        ));
    }
    let speaks = recon
        .views
        .iter()
        .filter(|v| {
            v.kind == ViewKind::Span
                && v.key == span.span
                && (v.trace.is_empty() || v.trace == span.trace)
        })
        .any(|v| v.blocks.iter().any(|b| b.output));
    if speaks {
        return Proof::Present(format!("{} records output", span.name));
    }
    let own: Vec<&Carrier> = haystack
        .carriers
        .iter()
        .filter(|c| c.span.as_deref() == Some(span.span.as_str()))
        .filter(|c| c.trace.as_deref().is_none_or(|t| t == span.trace))
        .collect();
    if let Some(at) = haystack
        .undecoded
        .iter()
        .find(|at| own.iter().any(|c| at.starts_with(&c.root)))
    {
        return Proof::Unprovable(format!("{at} is encoded deeper than the search decodes"));
    }
    // What would tie the step to a response: any model response's id the truth knows, by value - this call's or
    // another's, either is a response recorded on the step; and any answer the truth knows, through the absence
    // search - whole, in pieces, or in part.
    let responses = response_ids(truth);
    if let Some((at, _)) = own
        .iter()
        .flat_map(|c| c.strings.iter())
        .find(|(_, s)| responses.iter().any(|id| s.contains(id)))
    {
        return Proof::Present(format!(
            "{} records a model response's id at {at}",
            span.name
        ));
    }
    let own_haystack = Haystack {
        carriers: own.iter().map(|c| (*c).clone()).collect(),
        undecoded: Vec::new(),
    };
    for answer in truth
        .facts
        .iter()
        .filter(|f| f.call.is_some() && f.kind == "text")
    {
        match super::prove(answer, &own_haystack) {
            Proof::Absent => {}
            // An answer too short to search for cannot be shown absent from the step either.
            Proof::Unprovable(why) => return Proof::Unprovable(format!("{}: {why}", answer.id)),
            _ => return Proof::Present(format!("{} records {}'s answer", span.name, answer.id)),
        }
    }
    let known: Vec<&Value> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_call")
        .filter_map(|f| f.value.get("arguments"))
        .collect();
    // Every known call's identity, not only this call's: another call's id on the step records that call.
    let identities: Vec<&str> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_call")
        .filter_map(super::wire_id)
        .filter(|id| !id.is_empty())
        .collect();
    if let Some((at, _)) = own
        .iter()
        .flat_map(|c| c.strings.iter())
        .find(|(_, s)| identities.iter().any(|id| s.contains(id)))
    {
        return Proof::Present(format!("{} records a call's id at {at}", span.name));
    }
    if let Some(at) = records_the_call(&own, wire_id, &known) {
        return Proof::Present(format!("{} records the call at {at}", span.name));
    }
    Proof::Absent
}

/// Every model response id the truth knows.
fn response_ids(truth: &Truth) -> Vec<&str> {
    truth
        .calls
        .iter()
        .filter_map(|c| c.response_id.as_deref())
        .filter(|id| !id.is_empty())
        .collect()
}

/// Why another span could be the call's own, if one could: a raw carrier anywhere in the capture, or a reconstructed
/// span, stating the call's response id; or a span the reading types a model call that no call is tied to.
fn identified_elsewhere(
    call: &crate::message_truth::truth::Call,
    recon: &Recon,
    matching: &Matching,
    haystack: &Haystack,
) -> Option<String> {
    if let Some(id) = call.response_id.as_deref().filter(|id| !id.is_empty()) {
        if let Some((at, _)) = haystack
            .carriers
            .iter()
            .flat_map(|c| c.strings.iter())
            .find(|(_, s)| s.contains(id))
        {
            return Some(format!("{at} records {}'s response id", call.id));
        }
        if let Some(g) = recon
            .generations
            .iter()
            .find(|g| g.response_id.as_deref() == Some(id))
        {
            return Some(format!("{} states {}'s response id", g.label, call.id));
        }
    }
    let tied: BTreeSet<(&str, &str)> = matching
        .span_of
        .values()
        .map(|&g| {
            (
                recon.generations[g].trace.as_str(),
                recon.generations[g].span.as_str(),
            )
        })
        .collect();
    recon
        .generations
        .iter()
        .find(|g| g.typed && !tied.contains(&(g.trace.as_str(), g.span.as_str())))
        .map(|g| format!("{} is a model-call span no call is tied to", g.label))
}

/// Whether a location is the tool's own record of its arguments: the step's attribute itself, or a value decoded out
/// of it - never an event's member or a log body that happens to share its name.
fn tool_record(root: &str, at: &str) -> bool {
    at.strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_prefix(ARGUMENTS))
        .is_some_and(|rest| {
            !rest.starts_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | '['))
        })
}

/// Where a step's own carriers record a model's output, if they do: the call's wire id in any string, a published
/// response identifier under any key, or any known call's arguments anywhere but the tool's own record of them - a
/// function call object, a response body.
pub(super) fn records_the_call<'c>(
    carriers: &[&'c Carrier],
    wire_id: Option<&str>,
    known: &[&Value],
) -> Option<&'c str> {
    const IDENTIFIERS: &[&str] = &["gen_ai.response.id", "llm.response.id"];
    for carrier in carriers {
        if let Some((at, _)) = carrier.strings.iter().find(|(at, s)| {
            wire_id.is_some_and(|id| s.contains(id)) || IDENTIFIERS.iter().any(|k| at.contains(k))
        }) {
            return Some(at);
        }
        if let Some((at, _)) = carrier.nodes.iter().find(|(at, node)| {
            !tool_record(&carrier.root, at)
                && known
                    .iter()
                    .any(|arguments| crate::message_truth::predicates::json_eq(node, arguments))
        }) {
            return Some(at);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "adk-java/native@0.9.0/tool_use";

    fn capture() -> (Truth, Recon, Matching, Haystack) {
        let paths = crate::discover_fixtures()
            .into_iter()
            .find(|(label, _)| label == FIXTURE)
            .map(|(_, paths)| paths)
            .expect("the capture");
        let truth =
            crate::message_truth::truth::load_all()["adk-java/tool_use"].for_fixture(FIXTURE);
        let recon = crate::message_truth::recon::build(FIXTURE, &paths);
        let matching = crate::message_truth::matching::match_calls(&truth, &recon, &mut Vec::new());
        let haystack = Haystack::of_fixture(&recon.paths);
        (truth, recon, matching, haystack)
    }

    /// Each declared call is proven, and a normal model-call step - the last call's, which records its answer - is
    /// refused.
    #[test]
    fn an_overwritten_call_is_proven_and_a_normal_step_refused() {
        let (truth, recon, matching, haystack) = capture();
        let declared: Vec<&str> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "call_span_overwritten_by_tool" && g.holds_for(FIXTURE))
            .filter_map(|g| g.subject.as_deref())
            .collect();
        assert_eq!(declared.len(), 4, "the four calls that only call tools");
        for call in &declared {
            assert!(
                matches!(
                    prove_overwritten(&truth, call, &recon, &matching, &haystack),
                    Proof::Absent
                ),
                "{call} is not proven"
            );
        }
        let answering = truth.calls.last().expect("a call").id.clone();
        assert!(!matches!(
            prove_overwritten(&truth, &answering, &recon, &matching, &haystack),
            Proof::Absent
        ));
    }

    /// Every clause of the shape refuses on its own: another name, no tool execution, another tool, other arguments,
    /// a request or a response that is not emptied.
    #[test]
    fn each_clause_of_the_shape_refuses() {
        let (truth, recon, matching, _) = capture();
        let fact = truth
            .facts
            .iter()
            .find(|f| f.kind == "tool_call" && f.call.as_deref() == Some("call-001"))
            .expect("call-001's tool call");
        let (tool, arguments) = (
            fact.value["name"].as_str().expect("a name"),
            &fact.value["arguments"],
        );
        let steps: BTreeSet<String> = ["call_llm".to_string()].into();
        let span = stated_spans(&recon)
            .into_iter()
            .find(|s| shape_refusal(s, tool, arguments, &steps).is_none())
            .expect("the overwritten step");
        let _ = &matching;
        let altered = |edit: &dyn Fn(&mut Stated)| {
            let mut mutated = span.clone();
            edit(&mut mutated);
            shape_refusal(&mutated, tool, arguments, &steps).is_some()
        };
        assert_eq!(
            beyond_the_step(&span),
            None,
            "the overwritten step carries only the tool's execution"
        );
        let mut more = span.clone();
        more.attributes
            .insert("gen_ai.completion".into(), "Paris will be sunny".into());
        assert!(beyond_the_step(&more).is_some(), "another attribute");
        let mut evented = span.clone();
        evented.events = 1;
        assert!(beyond_the_step(&evented).is_some(), "an event");
        let mut twice = span.clone();
        twice.repeated = true;
        assert!(
            beyond_the_step(&twice).is_some(),
            "an attribute written twice"
        );
        let mut linked = span.clone();
        linked.links = 1;
        assert!(beyond_the_step(&linked).is_some(), "a link");
        let mut statused = span.clone();
        statused.status = "Paris will be sunny".into();
        assert!(beyond_the_step(&statused).is_some(), "a status message");
        assert!(
            altered(&|s| s.name = "execute_tool get_weather".into()),
            "another name"
        );
        assert!(
            altered(&|s| {
                s.attributes.remove("gen_ai.operation.name");
            }),
            "no tool execution"
        );
        assert!(
            altered(&|s| {
                s.attributes
                    .insert("gen_ai.operation.name".into(), "call_llm".into());
            }),
            "a normal step"
        );
        assert!(
            altered(&|s| {
                s.attributes
                    .insert("gen_ai.tool.name".into(), "book_flight".into());
            }),
            "another tool"
        );
        assert!(
            altered(&|s| {
                s.attributes
                    .insert(ARGUMENTS.into(), r#"{"city":"Rome"}"#.into());
            }),
            "other arguments"
        );
        assert!(
            altered(&|s| {
                s.attributes
                    .insert(REQUEST.into(), r#"{"contents":[]}"#.into());
            }),
            "a request"
        );
        assert!(
            altered(&|s| {
                s.attributes
                    .insert(RESPONSE.into(), r#"{"content":{}}"#.into());
            }),
            "a response"
        );
    }

    /// The step's carriers are searched as the producer wrote them: the call's id in any string, a response
    /// identifier under any key, or the arguments outside the tool's own record of them each refuse the proof.
    #[test]
    fn a_step_recording_the_call_anywhere_is_refused() {
        let arguments = serde_json::json!({"city": "Paris", "days": 2});
        let other = serde_json::json!({"city": "Tokyo", "days": 2});
        let root = r#"req-001.pb span "call_llm""#;
        let carrier = |strings: Vec<(String, &str)>, nodes: Vec<(String, Value)>| Carrier {
            root: root.to_string(),
            strings: strings
                .into_iter()
                .map(|(a, s)| (a, s.to_string()))
                .collect(),
            nodes,
            ..Default::default()
        };
        let tool_record = carrier(
            vec![(format!("{root}.{ARGUMENTS}.city"), "Paris")],
            vec![(format!("{root}.{ARGUMENTS}"), arguments.clone())],
        );
        assert_eq!(
            records_the_call(&[&tool_record], Some("toolu_1"), &[&arguments]),
            None,
            "the tool's own record"
        );
        for (what, recording) in [
            (
                "its id in an attribute",
                carrier(vec![(format!("{root}.note"), "called toolu_1")], vec![]),
            ),
            (
                "its id in an event",
                carrier(vec![(format!("{root}.events[0].body"), "toolu_1")], vec![]),
            ),
            (
                "a response id",
                carrier(
                    vec![(format!("{root}.gen_ai.response.id"), "msg_1")],
                    vec![],
                ),
            ),
            (
                "a function call holding its arguments",
                carrier(
                    vec![],
                    vec![(
                        format!("{root}.gcp.vertex.agent.data.functionCall.args"),
                        arguments.clone(),
                    )],
                ),
            ),
            (
                "another call's arguments in the tool's description",
                carrier(
                    vec![],
                    vec![(
                        format!("{root}.gen_ai.tool.description.functionCall.args"),
                        other.clone(),
                    )],
                ),
            ),
            (
                "an event member named like the tool's record",
                carrier(
                    vec![],
                    vec![(
                        format!("{root}.events[0].{ARGUMENTS}.functionCall.args"),
                        arguments.clone(),
                    )],
                ),
            ),
        ] {
            assert!(
                records_the_call(
                    &[&tool_record, &recording],
                    Some("toolu_1"),
                    &[&arguments, &other]
                )
                .is_some(),
                "{what} passed"
            );
        }
    }

    /// Another call's response recorded on the step, where the allowlist lets a value through, refuses the proof: any
    /// model response id the truth knows is searched, not only the call's own.
    #[test]
    fn another_call_s_response_id_on_the_step_is_refused() {
        let (truth, recon, matching, haystack) = capture();
        let candidate = overwritten_step(&truth, &recon);
        let other = truth.calls[1]
            .response_id
            .clone()
            .expect("call-002's response id");
        let mut recorded = haystack.clone();
        let carrier = recorded
            .carriers
            .iter_mut()
            .find(|c| c.span.as_deref() == Some(candidate.as_str()) && !c.record)
            .expect("the step's carrier");
        carrier.strings.push((
            format!("{}.gen_ai.tool.description", carrier.root),
            format!(r#"{{"response":{{"id":"{other}","stop_reason":"tool_use"}}}}"#),
        ));
        assert!(matches!(
            prove_overwritten(&truth, "call-001", &recon, &matching, &haystack),
            Proof::Absent
        ));
        assert!(
            matches!(
                prove_overwritten(&truth, "call-001", &recon, &matching, &recorded),
                Proof::Present(_)
            ),
            "another call's response id on the step passed"
        );
    }

    /// A span of the call's own beside the overwritten step refuses the proof, however little of it the reading kept:
    /// its response id in any raw carrier, a reconstructed span stating it, or a model-call span no call is tied to.
    #[test]
    fn a_normal_response_span_beside_the_overwritten_step_is_refused() {
        let (truth, recon, matching, haystack) = capture();
        let own = truth.calls[0]
            .response_id
            .clone()
            .expect("call-001's response id");
        let mut raw = haystack.clone();
        let carrier = raw.carriers.last_mut().expect("a carrier");
        carrier
            .strings
            .push((format!("{}.gen_ai.response.id", carrier.root), own.clone()));
        let template = recon.generations[matching.span_of["call-005"]].clone();
        let beside = |response_id: Option<String>| {
            let mut with = recon.clone();
            with.generations
                .push(super::super::super::recon::Generation {
                    label: "trace-1/call_llm/beside".into(),
                    span: "beside".into(),
                    typed: true,
                    response_id,
                    ..template.clone()
                });
            with
        };
        for (what, recon, haystack) in [
            ("its response id in a raw carrier", recon.clone(), raw),
            (
                "a span stating its response id",
                beside(Some(own.clone())),
                haystack.clone(),
            ),
            (
                "a model-call span no call is tied to",
                beside(None),
                haystack.clone(),
            ),
        ] {
            assert!(
                matches!(
                    prove_overwritten(&truth, "call-001", &recon, &matching, &haystack),
                    Proof::Present(_)
                ),
                "{what} passed"
            );
        }
    }

    /// The span that states call-001's overwrite.
    fn overwritten_step(truth: &Truth, recon: &Recon) -> String {
        let fact = truth
            .facts
            .iter()
            .find(|f| f.kind == "tool_call" && f.call.as_deref() == Some("call-001"))
            .expect("call-001's tool call");
        let steps: BTreeSet<String> = ["call_llm".to_string()].into();
        stated_spans(recon)
            .into_iter()
            .find(|s| {
                shape_refusal(
                    s,
                    fact.value["name"].as_str().unwrap(),
                    &fact.value["arguments"],
                    &steps,
                )
                .is_none()
            })
            .expect("the overwritten step")
            .span
    }

    /// The last step's view still owes the parts of the overwritten calls its request carries: deleted from that
    /// view alone, one is missing there.
    #[test]
    fn the_answering_step_still_owes_the_parts_it_was_sent() {
        let (truth, recon, baseline) = crate::message_truth::mutations::baseline(FIXTURE);
        let fact = truth
            .facts
            .iter()
            .find(|f| f.kind == "tool_call" && f.call.as_deref() == Some("call-001"))
            .expect("call-001's tool call");
        let mut deleted = recon.clone();
        let mut removed = false;
        for view in deleted
            .views
            .iter_mut()
            .filter(|v| v.kind == ViewKind::Span)
        {
            let before = view.blocks.len();
            view.blocks
                .retain(|b| b.tool_use_id.as_deref() != fact.value["id"].as_str());
            removed |= view.blocks.len() != before;
        }
        assert!(removed, "a span view shows the call");
        let new = crate::message_truth::mutations::added(&truth, &deleted, &baseline);
        assert!(
            new.iter().any(|a| a.ends_with(&format!(":{}", fact.id))),
            "the call deleted from its span views passed: {new:?}"
        );
    }
}
