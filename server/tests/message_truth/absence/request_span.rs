//! A model call whose request span records nothing of it (`call_span_records_no_output`): the producer
//! exports a span for the request, but that span carries none of the call's output and nothing identifying
//! the call, so no span can be tied to the call. Narrower than `call_span_not_exported`, which says no
//! model-call span exists and the call shares a run's span; this says the spans exist and are silent.

use super::haystack::Haystack;
use super::{Proof, prove};
use crate::message_truth::matching::signature;
use crate::message_truth::recon::{Recon, ViewKind};
use crate::message_truth::truth::Truth;

/// The published conventions' identifiers of one model response. A request span carrying any of them could
/// be tied to its call by that value, so it is not silent.
const IDENTIFIERS: &[&str] = &["gen_ai.response.id", "llm.response.id"];

/// `call_span_records_no_output` holds where the call succeeded with asserted output, the capture exports at
/// least one model-request span - the reading recognises it as a model call - and every one of them is
/// silent about the call:
///
/// - no output block in its view (looked up by trace and span, since span ids repeat across traces);
/// - no asserted output of any call, and no published response identifier, in its raw carriers - its
///   attributes and events, and every log record it emitted - so a parsing defect is never taken for silence;
/// - no response id read from it, and no usage that ties it to the call: no read output-token count, and no
///   number anywhere in its raw carriers, equal to the call's - by value, under any name or encoding;
/// - and every output it is searched for can be searched for: an unsearchable one refuses the proof.
///
/// A span a published convention marks as a model call that the reading does not recognise refuses the
/// proof, since its silence was never read; so does a capture with no model-request span at all, which is
/// `call_span_not_exported`'s statement, not this one's. The whole capture, not the call's trace alone: a
/// wider candidate set can only make the proof harder to pass.
pub(super) fn prove_request_span_silent(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    haystack: &Haystack,
) -> Proof {
    let Some(call) = truth.calls.iter().find(|c| c.id == call_id) else {
        return Proof::Unprovable(format!("no call {call_id}"));
    };
    if !call.succeeded() || call.outputs.is_empty() {
        return Proof::Unprovable(format!("{call_id} produced no output"));
    }
    let owed = signature(truth, call);
    if owed.is_empty() {
        return Proof::Unprovable(format!("{call_id} has no asserted output to look for"));
    }
    for (trace, span, label) in marked_model_call_spans(&recon.paths) {
        // Recognised as a model call, not merely reconstructed: a marked span the reading left untyped is skipped
        // as a candidate below, so its silence would go unread.
        if !recon
            .generations
            .iter()
            .any(|g| g.span == span && g.trace == trace && g.typed)
        {
            return Proof::Unprovable(format!(
                "{label} is a model-call span the reading does not recognise"
            ));
        }
    }
    // Recognition must be consistent, not merely present: the candidates come from the reading's typing, so every
    // raw span named like a recognised request span must be recognised too. One the reading left untyped would
    // be skipped below, its silence unread - a request span that carried the call's identity would vanish.
    let typed: std::collections::BTreeSet<(&str, &str)> = recon
        .generations
        .iter()
        .filter(|g| g.typed)
        .map(|g| (g.trace.as_str(), g.span.as_str()))
        .collect();
    let spans = raw_spans(&recon.paths);
    let request_names: std::collections::BTreeSet<&str> = spans
        .iter()
        .filter(|(trace, span, _)| typed.contains(&(trace.as_str(), span.as_str())))
        .map(|(_, _, name)| name.as_str())
        .collect();
    if let Some((_, _, name)) = spans.iter().find(|(trace, span, name)| {
        request_names.contains(name.as_str()) && !typed.contains(&(trace.as_str(), span.as_str()))
    }) {
        return Proof::Unprovable(format!(
            "a span named {name:?} like a recognised request span is not recognised itself"
        ));
    }
    if recon.generations.iter().all(|g| !g.typed) {
        return Proof::Unprovable(
            "no model-request span is exported, which is `call_span_not_exported`".to_string(),
        );
    }
    if let Some(at) = haystack.undecoded.first() {
        return Proof::Unprovable(format!("{at} is encoded deeper than the search decodes"));
    }
    let produced = call
        .usage
        .as_ref()
        .and_then(|u| u.output)
        .filter(|&n| n > 0);
    // The candidates: the spans the reading recognises as model calls. A span that is not one - Codex's
    // output spans, a tool span - records the response by being where it is reported, which says nothing
    // about whether the request span could be tied to the call.
    for generation in recon.generations.iter().filter(|g| g.typed) {
        if let Some(id) = &generation.response_id {
            return Proof::Present(format!("{} carries response id {id}", generation.label));
        }
        if !generation.failed && produced == Some(generation.output) {
            return Proof::Present(format!(
                "{} reports the call's {} output tokens, which tie it to the call",
                generation.label, generation.output
            ));
        }
        let speaks = recon
            .views
            .iter()
            .filter(|v| v.kind == ViewKind::Span && v.key == generation.span)
            .filter(|v| v.trace.is_empty() || v.trace == generation.trace)
            .any(|v| v.blocks.iter().any(|b| b.output));
        if speaks {
            return Proof::Present(format!("{} records output", generation.label));
        }
        // Its own raw carriers: the span with its events, and every log record it emitted - by trace and
        // span, since span ids repeat across traces.
        let own = Haystack {
            carriers: haystack
                .carriers
                .iter()
                .filter(|c| c.span.as_deref() == Some(generation.span.as_str()))
                .filter(|c| c.trace.as_deref().is_none_or(|t| t == generation.trace))
                .cloned()
                .collect(),
            undecoded: Vec::new(),
        };
        // A count in the raw carriers, read or not, under any name: any number in the span's own carriers equal
        // to the call's output tokens could tie it to the call, so silence cannot be shown. Matched by value, not
        // by attribute spelling - a counter can be written under any key, inside a decoded body, or as text - so a
        // coincidental equal number refuses the proof too, which keeps the violation rather than hiding one.
        if let Some(tokens) = produced
            && let Some((at, _)) = own
                .carriers
                .iter()
                .flat_map(|c| c.numbers.iter())
                .find(|(_, n)| *n == tokens)
        {
            return Proof::Present(format!(
                "{} carries {at} = {tokens}, the call's output tokens",
                generation.label
            ));
        }
        // A figure too large to be read surely may be that count: silence cannot be shown either.
        if produced.is_some()
            && let Some(at) = own.carriers.iter().find_map(|c| c.inexact.first())
        {
            return Proof::Unprovable(format!(
                "{} carries {at}, a figure too large to read surely, which may be the call's output tokens",
                generation.label
            ));
        }
        if let Some((at, _)) = own
            .carriers
            .iter()
            .flat_map(|c| c.strings.iter())
            .find(|(at, _)| IDENTIFIERS.iter().any(|key| at.contains(key)))
        {
            return Proof::Present(format!("{} carries {at}", generation.label));
        }
        // The call's own response id, by value under any key: a carrier holding it identifies the call
        // whatever the member is called.
        if let Some(id) = call.response_id.as_deref() {
            // An id too short to search for distinctively cannot be shown absent.
            if id.len() < 6 {
                return Proof::Unprovable(format!(
                    "{call_id}'s response id {id:?} is too short to search for"
                ));
            }
        }
        if let Some(id) = call.response_id.as_deref()
            && let Some((at, _)) = own
                .carriers
                .iter()
                .flat_map(|c| c.strings.iter())
                .find(|(_, value)| value.contains(id))
        {
            return Proof::Present(format!(
                "{} carries {at}, the call's response id",
                generation.label
            ));
        }
        // Any response's asserted output, not only this call's: a request span recording a response is not
        // silent, whichever call it belongs to.
        for fact in truth
            .facts
            .iter()
            .filter(|f| f.call.is_some() && f.require.is_some())
        {
            match prove(fact, &own) {
                Proof::Absent => {}
                Proof::Unprovable(why) => {
                    return Proof::Unprovable(format!(
                        "{}: {} cannot be searched for: {why}",
                        generation.label, fact.id
                    ));
                }
                Proof::Present(_) | Proof::Partial(_) => {
                    return Proof::Present(format!(
                        "{}'s raw carriers hold {}",
                        generation.label, fact.id
                    ));
                }
            }
        }
    }
    Proof::Absent
}

/// Every span of the capture, as (trace, span) in hex with its name.
fn raw_spans(paths: &[std::path::PathBuf]) -> Vec<(String, String, String)> {
    paths
        .iter()
        .flat_map(|path| {
            crate::decode_request(path)
                .resource_spans
                .into_iter()
                .flat_map(|r| r.scope_spans)
                .flat_map(|s| s.spans)
                .map(|span| {
                    (
                        super::haystack::hex(&span.trace_id),
                        super::haystack::hex(&span.span_id),
                        span.name,
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Every span of the capture a published convention marks as a model call: its trace and span, as hex, and
/// its name.
pub(super) fn marked_model_call_spans(
    paths: &[std::path::PathBuf],
) -> Vec<(String, String, String)> {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    const MODEL_OPERATIONS: &[&str] = &["chat", "text_completion", "generate_content"];
    let mut found = Vec::new();
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
                found.push((
                    super::haystack::hex(&span.trace_id),
                    super::haystack::hex(&span.span_id),
                    format!("span {:?}", span.name),
                ));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::super::Proof;
    use super::super::haystack::{Carrier, Haystack};
    use super::prove_request_span_silent;
    use crate::message_truth::mutations::baseline;
    use crate::message_truth::recon::{Recon, View, ViewKind};
    use crate::message_truth::truth::Truth;

    const FIXTURE: &str = "codex/native/tool_use";

    fn haystack(fixture: &str) -> Haystack {
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        Haystack::of_fixture(&fixtures[fixture])
    }

    /// The calls the fixture declares `call_span_records_no_output`.
    fn silent_calls(truth: &Truth) -> Vec<String> {
        let calls: Vec<String> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "call_span_records_no_output" && g.holds_for(FIXTURE))
            .filter_map(|g| g.subject.clone())
            .collect();
        assert!(!calls.is_empty(), "{FIXTURE} declares silent request spans");
        calls
    }

    /// An assistant text the trace view shows: an output of some call.
    fn an_output(recon: &Recon) -> crate::message_truth::recon::Block {
        recon
            .views
            .iter()
            .filter(|v| v.kind == ViewKind::Trace)
            .flat_map(|v| v.blocks.iter())
            .find(|b| b.role == "assistant")
            .cloned()
            .expect("the trace view shows an assistant block")
    }

    fn refused(truth: &Truth, calls: &[String], recon: &Recon, haystack: &Haystack, what: &str) {
        for call in calls {
            assert!(
                matches!(
                    prove_request_span_silent(truth, call, recon, haystack),
                    Proof::Present(_)
                ),
                "{what}: {call} must not be proven silent"
            );
        }
    }

    /// Proven on the capture as recorded. Each mutation then gives one request span exactly one thing that
    /// could tie it to its call - in the reading, in its raw carriers or linked logs, or in its usage - and
    /// the proof must refuse, so the entry stays a violation.
    #[test]
    fn a_silent_request_span_is_proven_only_while_every_candidate_stays_silent() {
        let (truth, recon, _) = baseline(FIXTURE);
        let found = haystack(FIXTURE);
        let calls = silent_calls(&truth);
        for call in &calls {
            assert_eq!(
                prove_request_span_silent(&truth, call, &recon, &found),
                Proof::Absent,
                "{call} is proven on the capture as recorded"
            );
        }
        let at = recon
            .generations
            .iter()
            .position(|g| g.typed)
            .expect("the capture's request spans are recognised");
        let span = recon.generations[at].span.clone();

        let mut identified = recon.clone();
        identified.generations[at].response_id = Some("resp_1".to_string());
        refused(
            &truth,
            &calls,
            &identified,
            &found,
            "a response id read from the span",
        );

        let mut speaking = recon.clone();
        let output = an_output(&recon);
        speaking
            .views
            .iter_mut()
            .find(|v| v.kind == ViewKind::Span && v.key == span)
            .expect("the request span has a view")
            .blocks
            .push(crate::message_truth::recon::Block {
                output: true,
                span: span.clone(),
                ..output.clone()
            });
        refused(
            &truth,
            &calls,
            &speaking,
            &found,
            "an output block in the span's view",
        );

        // The raw carriers say what the reading did not: a parsing defect is never silence.
        let text = truth
            .facts
            .iter()
            .find(|f| f.kind == "text" && f.require.is_some())
            .map(|f| f.text().to_string())
            .expect("an asserted answer");
        for (location, value) in [
            ("span.attributes.gen_ai.output.messages", text.clone()),
            ("log[3].body.response", text.clone()),
            ("span.attributes.gen_ai.response.id", "resp_1".to_string()),
        ] {
            let mut raw = haystack(FIXTURE);
            raw.carriers.push(Carrier {
                span: Some(span.clone()),
                strings: vec![(location.to_string(), value)],
                ..Carrier::default()
            });
            refused(&truth, &calls, &recon, &raw, location);
        }

        // Usage equal to a call's output tokens is what metadata matching ties a span to its call by.
        for call in &calls {
            let Some(tokens) = truth
                .calls
                .iter()
                .find(|c| &c.id == call)
                .and_then(|c| c.usage.as_ref())
                .and_then(|u| u.output)
                .filter(|&n| n > 0)
            else {
                continue;
            };
            let mut tied = recon.clone();
            tied.generations[at].output = tokens;
            assert!(
                matches!(
                    prove_request_span_silent(&truth, call, &tied, &found),
                    Proof::Present(_)
                ),
                "{call}: usage tying the span to the call must refuse the proof"
            );
        }

        // A span id another trace reuses: the other trace's span speaks, and must not hide behind this one.
        let mut reused = recon.clone();
        let mut twin = reused.generations[at].clone();
        twin.trace = "another-trace".to_string();
        reused.generations.push(twin);
        reused.views.push(View::plain_span(
            "another-trace".to_string(),
            span.clone(),
            vec![crate::message_truth::recon::Block {
                output: true,
                span: span.clone(),
                ..output
            }],
        ));
        refused(
            &truth,
            &calls,
            &reused,
            &found,
            "a speaking span reusing the id in another trace",
        );
    }

    /// Usage the reading dropped is still usage: a raw output-token count equal to the call's, in a request
    /// span's own carriers, ties the span to the call. And an output that cannot be searched for - a two-letter
    /// answer - leaves silence unproven rather than assumed.
    #[test]
    fn raw_usage_and_an_unsearchable_output_refuse_the_proof() {
        let (truth, recon, _) = baseline(FIXTURE);
        let calls = silent_calls(&truth);
        let generation = recon
            .generations
            .iter()
            .find(|g| g.typed)
            .expect("a recognised request span")
            .clone();
        for call in &calls {
            let Some(tokens) = truth
                .calls
                .iter()
                .find(|c| &c.id == call)
                .and_then(|c| c.usage.as_ref())
                .and_then(|u| u.output)
                .filter(|&n| n > 0)
            else {
                continue;
            };
            let mut raw = haystack(FIXTURE);
            raw.carriers.push(Carrier {
                trace: Some(generation.trace.clone()),
                span: Some(generation.span.clone()),
                numbers: vec![(
                    "log[2] \"codex.api_request\".gen_ai.usage.output_tokens".to_string(),
                    tokens,
                )],
                ..Carrier::default()
            });
            assert!(
                matches!(
                    prove_request_span_silent(&truth, call, &recon, &raw),
                    Proof::Present(_)
                ),
                "{call}: a raw output-token count equal to the call's must refuse the proof"
            );
        }
        let mut short = truth.clone();
        let mut answer = short
            .facts
            .iter()
            .find(|f| f.kind == "text" && f.call.is_some() && f.require.is_some())
            .cloned()
            .expect("an asserted answer");
        answer.id = "fact-short".to_string();
        answer.value = serde_json::json!({"text": "OK"});
        short.facts.push(answer);
        let found = haystack(FIXTURE);
        for call in &calls {
            assert!(
                matches!(
                    prove_request_span_silent(&short, call, &recon, &found),
                    Proof::Unprovable(_)
                ),
                "{call}: an output that cannot be searched for must leave silence unproven"
            );
        }
    }

    /// A count inside a decoded log body is still a count: a request span's linked log record whose body is
    /// JSON holding the call's output tokens ties the span to the call.
    #[test]
    fn an_encoded_usage_count_in_a_linked_log_refuses_the_proof() {
        use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
        use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
        use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
        let (truth, recon, _) = baseline(FIXTURE);
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        let paths = &fixtures[FIXTURE];
        let generation = recon
            .generations
            .iter()
            .find(|g| g.typed)
            .expect("a recognised request span")
            .clone();
        let unhex = |text: &str| -> Vec<u8> {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
                .collect()
        };
        let traces: Vec<(String, _)> = paths
            .iter()
            .map(|p| (p.display().to_string(), crate::decode_request(p)))
            .collect();
        let mut logs: Vec<(String, ExportLogsServiceRequest)> = crate::log_exports_beside(paths)
            .iter()
            .map(|p| (p.display().to_string(), crate::decode_logs(p)))
            .collect();
        for call in silent_calls(&truth) {
            let Some(tokens) = truth
                .calls
                .iter()
                .find(|c| c.id == call)
                .and_then(|c| c.usage.as_ref())
                .and_then(|u| u.output)
                .filter(|&n| n > 0)
            else {
                continue;
            };
            // Five encodings of the same count: an alias inside a JSON body, a nested usage object inside a JSON
            // body, the count as a text attribute, a whole-valued float in a body, and a double attribute.
            let text = |value: String| AnyValue {
                value: Some(any_value::Value::StringValue(value)),
            };
            let planted_records = [
                LogRecord {
                    body: Some(text(format!(
                        "{{\"gen_ai.usage.completion_tokens\": {tokens}}}"
                    ))),
                    ..LogRecord::default()
                },
                LogRecord {
                    body: Some(text(format!(
                        "{{\"response_data\": {{\"usage\": {{\"completion_tokens\": {tokens}}}}}}}"
                    ))),
                    ..LogRecord::default()
                },
                LogRecord {
                    attributes: vec![opentelemetry_proto::tonic::common::v1::KeyValue {
                        key: "gen_ai.usage.completion_tokens".to_string(),
                        value: Some(text(tokens.to_string())),
                    }],
                    ..LogRecord::default()
                },
                // A whole-valued float, in a body and as a double attribute.
                LogRecord {
                    body: Some(text(format!("{{\"usage\": {{\"output\": {tokens}.0}}}}"))),
                    ..LogRecord::default()
                },
                LogRecord {
                    attributes: vec![opentelemetry_proto::tonic::common::v1::KeyValue {
                        key: "tokens".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::DoubleValue(tokens as f64)),
                        }),
                    }],
                    ..LogRecord::default()
                },
            ];
            for mut record in planted_records {
                record.trace_id = unhex(&generation.trace);
                record.span_id = unhex(&generation.span);
                logs.push((
                    "logs-planted.pb".to_string(),
                    ExportLogsServiceRequest {
                        resource_logs: vec![ResourceLogs {
                            scope_logs: vec![ScopeLogs {
                                log_records: vec![record],
                                ..ScopeLogs::default()
                            }],
                            ..ResourceLogs::default()
                        }],
                    },
                ));
                let planted = Haystack::from_requests(&traces, &logs);
                logs.pop();
                assert!(
                    matches!(
                        prove_request_span_silent(&truth, &call, &recon, &planted),
                        Proof::Present(_)
                    ),
                    "{call}: an output-token count in a linked log, in any encoding, must refuse the proof"
                );
            }
        }
    }

    /// The call's own response id, under a key no convention names, identifies it: a request span whose raw
    /// carriers hold it is not silent.
    #[test]
    fn the_call_s_response_id_under_any_key_refuses_the_proof() {
        let (truth, recon, _) = baseline(FIXTURE);
        let generation = recon
            .generations
            .iter()
            .find(|g| g.typed)
            .expect("a recognised request span")
            .clone();
        for call in silent_calls(&truth) {
            let id = truth
                .calls
                .iter()
                .find(|c| c.id == call)
                .and_then(|c| c.response_id.clone())
                .expect("the call's response id");
            let mut raw = haystack(FIXTURE);
            raw.carriers.push(Carrier {
                trace: Some(generation.trace.clone()),
                span: Some(generation.span.clone()),
                strings: vec![("log[4].body|json.response.id".to_string(), id.clone())],
                ..Carrier::default()
            });
            assert!(
                matches!(
                    prove_request_span_silent(&truth, &call, &recon, &raw),
                    Proof::Present(_)
                ),
                "{call}: its response id under an unconventional key must refuse the proof"
            );
        }
    }

    /// The candidates are the reading's: a request span it failed to type would be skipped, so one span named
    /// like the recognised request spans but left untyped refuses the proof - even where it is the one carrying
    /// the call's identity.
    #[test]
    fn a_request_span_the_reading_failed_to_type_refuses_the_proof() {
        let (truth, recon, _) = baseline(FIXTURE);
        let typed: Vec<usize> = (0..recon.generations.len())
            .filter(|&i| recon.generations[i].typed)
            .collect();
        assert!(typed.len() >= 2, "{FIXTURE} has several request spans");
        let mut missed = recon.clone();
        missed.generations[typed[0]].typed = false;
        let generation = recon.generations[typed[0]].clone();
        for call in silent_calls(&truth) {
            let id = truth
                .calls
                .iter()
                .find(|c| c.id == call)
                .and_then(|c| c.response_id.clone())
                .expect("a response id");
            let mut raw = haystack(FIXTURE);
            raw.carriers.push(Carrier {
                trace: Some(generation.trace.clone()),
                span: Some(generation.span.clone()),
                strings: vec![("log[4].body|json.response.id".to_string(), id)],
                ..Carrier::default()
            });
            assert!(
                !matches!(
                    prove_request_span_silent(&truth, &call, &missed, &raw),
                    Proof::Absent
                ),
                "{call}: an untyped request span carrying the call's id must not leave silence proven"
            );
        }
    }

    /// A response id too short to search for distinctively leaves silence unproven.
    #[test]
    fn a_short_response_id_refuses_the_proof() {
        let (mut truth, recon, _) = baseline(FIXTURE);
        let found = haystack(FIXTURE);
        let call = silent_calls(&truth)[0].clone();
        truth
            .calls
            .iter_mut()
            .find(|c| c.id == call)
            .expect("the call")
            .response_id = Some("r1".to_string());
        assert!(
            matches!(
                prove_request_span_silent(&truth, &call, &recon, &found),
                Proof::Unprovable(_)
            ),
            "a two-letter response id cannot be shown absent"
        );
    }

    /// A silent request span frees the call from a span of its own, not its parts from every span view: the span
    /// whose payload records the answer owes it in its own view, so removing it there alone - trace and feed
    /// untouched - is still caught.
    #[test]
    fn a_part_removed_from_the_span_that_records_it_is_still_caught() {
        use crate::message_truth::mutations::added;
        let (truth, recon, baseline) = baseline(FIXTURE);
        let answer = truth
            .facts
            .iter()
            .find(|f| {
                f.kind == "text"
                    && f.require.is_some()
                    && f.call
                        .as_deref()
                        .is_some_and(|c| silent_calls(&truth).iter().any(|s| s == c))
            })
            .expect("a silent call's answer");
        let mut removed = recon.clone();
        let mut hit = false;
        for view in removed
            .views
            .iter_mut()
            .filter(|v| v.kind == ViewKind::Span)
        {
            let before = view.blocks.len();
            view.blocks.retain(|b| {
                crate::message_truth::predicates::shows(answer, b, None)
                    == crate::message_truth::predicates::Shows::No
            });
            hit |= view.blocks.len() != before;
        }
        assert!(hit, "a span view shows the answer");
        let new = added(&truth, &removed, &baseline);
        assert!(
            new.contains(&format!("text.missing:{}", answer.id)),
            "the answer missing from the span recording it is caught: {new:?}"
        );
    }

    /// A carrier span owes what it records and shows nothing else: text added only to the view of the span
    /// recording the answer - every other view untouched - is caught as content nothing explains.
    #[test]
    fn invented_content_on_a_carrier_span_is_still_caught() {
        use crate::message_truth::mutations::added;
        let (truth, recon, baseline) = baseline(FIXTURE);
        let answer = truth
            .facts
            .iter()
            .find(|f| {
                f.kind == "text"
                    && f.require.is_some()
                    && f.call
                        .as_deref()
                        .is_some_and(|c| silent_calls(&truth).iter().any(|s| s == c))
            })
            .expect("a silent call's answer");
        let mut invented = recon.clone();
        let view = invented
            .views
            .iter_mut()
            .filter(|v| v.kind == ViewKind::Span)
            .find(|v| {
                v.blocks.iter().any(|b| {
                    crate::message_truth::predicates::shows(answer, b, None)
                        != crate::message_truth::predicates::Shows::No
                })
            })
            .expect("the span view recording the answer");
        let mut block = view.blocks[0].clone();
        block.content =
            serde_json::json!({"type": "text", "text": "an answer the model never gave"});
        block.digest = "invented".to_string();
        block.identity = "invented".to_string();
        view.blocks.push(block);
        let new = added(&truth, &invented, &baseline);
        assert!(
            new.iter().any(|v| v.starts_with("extra.")),
            "invented content on the span recording the answer is caught: {new:?}"
        );
    }

    /// A span a convention marks as a model call that the reading leaves untyped is skipped as a candidate,
    /// so its silence was never read: the proof refuses.
    #[test]
    fn a_marked_span_the_reading_leaves_untyped_refuses_the_proof() {
        let fixture = "claude-code/logs/tool_use";
        let (truth, recon, _) = baseline(fixture);
        let found = haystack(fixture);
        // One marked span left untyped while another stays typed and silent, so the refusal is the guard's and
        // not the "no request span" check's.
        let mut untyped = recon.clone();
        let typed: Vec<usize> = (0..untyped.generations.len())
            .filter(|&i| untyped.generations[i].typed)
            .collect();
        assert!(typed.len() >= 2, "two request spans to keep one typed");
        untyped.generations[typed[0]].typed = false;
        let call = truth
            .calls
            .iter()
            .find(|c| c.succeeded() && !c.outputs.is_empty())
            .map(|c| c.id.clone())
            .expect("a call with output");
        assert!(
            matches!(
                prove_request_span_silent(&truth, &call, &untyped, &found),
                Proof::Unprovable(_)
            ),
            "a marked span the reading left untyped must refuse the proof"
        );
    }

    /// Silence needs a span to be silent: with no model-request span the proof is not this gap's to give.
    #[test]
    fn no_exported_request_span_refuses_the_proof() {
        let (truth, recon, _) = baseline(FIXTURE);
        let found = haystack(FIXTURE);
        let mut none = recon.clone();
        none.generations.retain(|g| !g.typed);
        for call in silent_calls(&truth) {
            assert!(
                matches!(
                    prove_request_span_silent(&truth, &call, &none, &found),
                    Proof::Unprovable(_)
                ),
                "{call}: no request span to be silent must refuse the proof"
            );
        }
    }

    /// A span a published convention marks as a model call, that the reading does not recognise, has silence
    /// nobody read: the proof cannot rest on it.
    #[test]
    fn a_model_call_span_the_reading_does_not_recognise_refuses_the_proof() {
        let fixture = "claude-code/logs/tool_use";
        let (truth, recon, _) = baseline(fixture);
        let found = haystack(fixture);
        let mut unread = recon.clone();
        unread.generations.clear();
        let call = truth
            .calls
            .iter()
            .find(|c| c.succeeded() && !c.outputs.is_empty())
            .map(|c| c.id.clone())
            .expect("a call with output");
        assert!(
            matches!(
                prove_request_span_silent(&truth, &call, &unread, &found),
                Proof::Unprovable(_)
            ),
            "an unrecognised model-call span must refuse the proof"
        );
    }

    /// A count written as a float too large to be read surely refuses the proof as the exact count does: it may
    /// be the call's output tokens.
    #[test]
    fn a_count_too_large_to_read_surely_refuses_the_proof() {
        use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
        use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
        use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
        const LARGE: i64 = 2_251_799_813_685_248; // 2^51
        let (mut truth, recon, _) = baseline(FIXTURE);
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        let paths = &fixtures[FIXTURE];
        let generation = recon
            .generations
            .iter()
            .find(|g| g.typed)
            .expect("a recognised request span")
            .clone();
        let unhex = |text: &str| -> Vec<u8> {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
                .collect()
        };
        let traces: Vec<(String, _)> = paths
            .iter()
            .map(|p| (p.display().to_string(), crate::decode_request(p)))
            .collect();
        let call = silent_calls(&truth)
            .into_iter()
            .next()
            .expect("a silent call");
        if let Some(usage) = truth
            .calls
            .iter_mut()
            .find(|c| c.id == call)
            .and_then(|c| c.usage.as_mut())
        {
            usage.output = Some(LARGE);
        }
        let record = LogRecord {
            trace_id: unhex(&generation.trace),
            span_id: unhex(&generation.span),
            attributes: vec![KeyValue {
                key: "tokens".to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::DoubleValue(LARGE as f64)),
                }),
            }],
            ..LogRecord::default()
        };
        let logs = vec![(
            "logs-planted.pb".to_string(),
            ExportLogsServiceRequest {
                resource_logs: vec![ResourceLogs {
                    scope_logs: vec![ScopeLogs {
                        log_records: vec![record],
                        ..ScopeLogs::default()
                    }],
                    ..ResourceLogs::default()
                }],
            },
        )];
        let planted = Haystack::from_requests(&traces, &logs);
        assert_ne!(
            prove_request_span_silent(&truth, &call, &recon, &planted),
            Proof::Absent
        );
    }
}
