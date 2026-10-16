//! A call whose request span records none of its request (`input_not_exported`): the span the call is tied to
//! carries none of what the call was sent, so that span's view owes no input. The conversation views still
//! owe every part, wherever the telemetry records it.

use std::collections::BTreeMap;

use super::haystack::Haystack;
use super::{Proof, prove};
use crate::message_truth::matching::Matching;
use crate::message_truth::recon::Recon;
use crate::message_truth::requests;
use crate::message_truth::truth::Truth;

/// `input_not_exported` holds where the call is tied to a span, its recorded request sent at least one part,
/// and no raw carrier of that span holds any part *as the request sent it*. The span's carriers are its
/// attributes and events and every log record it emitted, matched by trace and span; the parts are every
/// occurrence, request-only content included, built exactly as the request check builds them. A part that
/// cannot be searched for refuses the proof.
pub(super) fn prove_input_unexported(
    truth: &Truth,
    call_id: &str,
    recon: &Recon,
    matching: &Matching,
    haystack: &Haystack,
) -> Proof {
    let Some(&generation) = matching.span_of.get(call_id) else {
        return Proof::Unprovable(format!("{call_id} is tied to no span"));
    };
    let generation = &recon.generations[generation];
    let Some(request) = truth
        .requests
        .get(&recon.fixture)
        .and_then(|r| r.calls.get(call_id))
    else {
        return Proof::Unprovable(format!("{call_id} has no recorded request"));
    };
    let marks = requests::marks(truth);
    let parts = requests::expected(call_id, request, &BTreeMap::new(), &marks);
    if parts.is_empty() {
        return Proof::Unprovable(format!("{call_id}'s request sent nothing to look for"));
    }
    let own = Haystack {
        carriers: haystack
            .carriers
            .iter()
            .filter(|c| c.span.as_deref() == Some(generation.span.as_str()))
            .filter(|c| c.trace.as_deref().is_none_or(|t| t == generation.trace))
            .cloned()
            .collect(),
        undecoded: haystack.undecoded.clone(),
    };
    for part in &parts {
        let sent = requests::as_fact(&part.label, part.role, &part.part, &BTreeMap::new())
            .unwrap_or_else(|| part.fact.clone());
        match prove(&sent, &own) {
            Proof::Absent => {}
            Proof::Unprovable(why) => {
                return Proof::Unprovable(format!("{}: {why}", part.label));
            }
            found => {
                return Proof::Present(format!(
                    "{} records {}: {found:?}",
                    generation.label, part.label
                ));
            }
        }
    }
    Proof::Absent
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::haystack::{Carrier, Haystack};
    use super::super::{Proof, carrying_spans};
    use super::prove_input_unexported;
    use crate::message_truth::matching::match_calls;
    use crate::message_truth::mutations::{added, baseline};
    use crate::message_truth::recon::ViewKind;

    const FIXTURE: &str = "claude-code/logs/tool_use";

    fn haystack(fixture: &str) -> Haystack {
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        Haystack::of_fixture(&fixtures[fixture])
    }

    /// Proven on the capture as recorded; refused when the tied span's carriers - or a log record it emitted -
    /// hold any part the request sent, request-only content included.
    #[test]
    fn a_request_span_s_silence_covers_every_part_the_request_sent() {
        let (truth, recon, _) = baseline(FIXTURE);
        let matching = match_calls(&truth, &recon, &mut Vec::new());
        let found = haystack(FIXTURE);
        let calls: Vec<String> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "input_not_exported" && g.holds_for(FIXTURE))
            .filter_map(|g| g.subject.clone())
            .filter(|c| matching.span_of.contains_key(c))
            .collect();
        assert!(
            !calls.is_empty(),
            "{FIXTURE} declares tied calls with unexported input"
        );
        for call in &calls {
            assert_eq!(
                prove_input_unexported(&truth, call, &recon, &matching, &found),
                Proof::Absent,
                "{call} is proven on the capture as recorded"
            );
            let request = &truth.requests[FIXTURE].calls[call];
            let generation = &recon.generations[matching.span_of[call]];
            // Every part's text, request-only ones (lineage `new`) included.
            for occurrence in request.messages.iter().flat_map(|m| m.parts.iter()) {
                let Some(text) = occurrence.part["text"]
                    .as_str()
                    .or(occurrence.part["content"].as_str())
                    .filter(|t| t.len() > 20)
                else {
                    continue;
                };
                let mut raw = haystack(FIXTURE);
                raw.carriers.push(Carrier {
                    trace: Some(generation.trace.clone()),
                    span: Some(generation.span.clone()),
                    strings: vec![("log[9].body".to_string(), text.to_string())],
                    ..Carrier::default()
                });
                assert!(
                    matches!(
                        prove_input_unexported(&truth, call, &recon, &matching, &raw),
                        Proof::Present(_)
                    ),
                    "{call}: a part the request sent, on its span, must refuse the proof"
                );
            }
        }
    }

    /// The gap waives a part's absence and nothing else: an input the request never sent, shown on the tied
    /// span's view, is still caught.
    #[test]
    fn an_invented_input_is_still_caught_under_the_input_gap() {
        let (truth, recon, baseline) = baseline(FIXTURE);
        let matching = match_calls(&truth, &recon, &mut Vec::new());
        let call = matching.span_of.keys().next().expect("a tied call").clone();
        let span = recon.generations[matching.span_of[&call]].span.clone();
        let template = recon
            .views
            .iter()
            .flat_map(|v| v.blocks.iter())
            .find(|b| b.role == "user")
            .cloned()
            .expect("a user block");
        let mut invented = recon.clone();
        let view = invented
            .views
            .iter_mut()
            .find(|v| v.kind == ViewKind::Span && v.key == span)
            .expect("the tied span's view");
        let mut block = template;
        block.output = false;
        block.span = span.clone();
        block.content =
            serde_json::json!({"type": "text", "text": "an input this request never sent"});
        block.digest = "invented".to_string();
        block.identity = "invented".to_string();
        view.blocks.push(block);
        let new = added(&truth, &invented, &baseline);
        assert!(
            new.iter().any(|v| v.starts_with("request.")),
            "an invented input on a span whose request is unexported is still caught: {new:?}"
        );
    }

    /// A quotation is not provenance, and a span is its trace's: only a carrier whose string *is* the text
    /// makes a home, and the home names the trace it is in.
    #[test]
    fn only_a_recorded_part_in_its_own_trace_makes_a_home() {
        let (truth, _, _) = baseline(FIXTURE);
        let answer = truth
            .facts
            .iter()
            .find(|f| f.kind == "text" && f.require.is_some())
            .expect("an answer");
        let text = answer.text().to_string();
        let haystack = Haystack {
            carriers: vec![
                Carrier {
                    trace: Some("aa".repeat(16)),
                    span: Some("bb".repeat(8)),
                    strings: vec![("debug.quote".to_string(), format!("the model said: {text}"))],
                    ..Carrier::default()
                },
                Carrier {
                    trace: Some("cc".repeat(16)),
                    span: Some("dd".repeat(8)),
                    strings: vec![("log[0].body.response".to_string(), text.clone())],
                    ..Carrier::default()
                },
            ],
            undecoded: Vec::new(),
        };
        let homes = carrying_spans(answer, &haystack, &Default::default());
        assert_eq!(
            homes.into_iter().collect::<Vec<_>>(),
            vec![("cc".repeat(16), "dd".repeat(8))],
            "a quotation is no home, and the home carries its trace"
        );
    }

    /// Two calls asking the same thing share their arguments, not their identity: a carrier recording the other
    /// call's id is no home for this one, and one recording this call's id is.
    #[test]
    fn a_tool_call_is_at_home_where_its_own_id_is() {
        let (truth, _, _) = baseline(FIXTURE);
        let call = truth
            .facts
            .iter()
            .find(|f| f.kind == "tool_call")
            .expect("a tool call");
        let own = super::super::wire_id(call).expect("a wire id").to_string();
        let carrier = |id: &str, span: &str| Carrier {
            trace: Some("ff".repeat(16)),
            span: Some(span.to_string()),
            strings: vec![("span.attributes.tool_use_id".to_string(), id.to_string())],
            nodes: vec![(
                "span.attributes.tool_input|json".to_string(),
                call.value["arguments"].clone(),
            )],
            ..Carrier::default()
        };
        let haystack = Haystack {
            carriers: vec![
                carrier("toolu_another_call_entirely", &"33".repeat(8)),
                carrier(&own, &"44".repeat(8)),
            ],
            undecoded: Vec::new(),
        };
        assert_eq!(
            carrying_spans(call, &haystack, &Default::default())
                .into_iter()
                .collect::<Vec<_>>(),
            vec![("ff".repeat(16), "44".repeat(8))],
        );
    }

    /// A tool's result repeating an answer is the call's record, not the answer's: a carrier holding some call's
    /// wire id beside the text is no home for the text; one holding the text alone is.
    #[test]
    fn a_call_s_record_repeating_an_answer_is_no_home_for_it() {
        let (truth, _, _) = baseline(FIXTURE);
        let answer = truth
            .facts
            .iter()
            .find(|f| f.kind == "text" && f.require.is_some())
            .expect("an answer");
        let text = answer.text().to_string();
        let call_ids: BTreeSet<&str> = ["call_0123456789"].into_iter().collect();
        let haystack = Haystack {
            carriers: vec![
                Carrier {
                    trace: Some("aa".repeat(16)),
                    span: Some("55".repeat(8)),
                    strings: vec![
                        ("log[1].call_id".to_string(), "call_0123456789".to_string()),
                        ("log[1].output".to_string(), text.clone()),
                    ],
                    ..Carrier::default()
                },
                Carrier {
                    trace: Some("aa".repeat(16)),
                    span: Some("66".repeat(8)),
                    strings: vec![("log[2].message".to_string(), text)],
                    ..Carrier::default()
                },
            ],
            undecoded: Vec::new(),
        };
        assert_eq!(
            carrying_spans(answer, &haystack, &call_ids)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![("aa".repeat(16), "66".repeat(8))],
        );
    }

    /// An answer whose own JSON mentions a call is still the answer: the id decoded out of the value holding the
    /// text does not make its carrier a call's record.
    #[test]
    fn an_answer_mentioning_a_call_keeps_its_home() {
        let (truth, _, _) = baseline(FIXTURE);
        let answer = truth
            .facts
            .iter()
            .find(|f| f.kind == "text" && f.require.is_some())
            .expect("an answer");
        let text = answer.text().to_string();
        let call_ids: BTreeSet<&str> = ["call_0123456789"].into_iter().collect();
        let haystack = Haystack {
            carriers: vec![Carrier {
                trace: Some("aa".repeat(16)),
                span: Some("77".repeat(8)),
                strings: vec![
                    ("log[3].response|json.answer".to_string(), text),
                    (
                        "log[3].response|json.last_tool_call".to_string(),
                        "call_0123456789".to_string(),
                    ),
                ],
                ..Carrier::default()
            }],
            undecoded: Vec::new(),
        };
        assert_eq!(
            carrying_spans(answer, &haystack, &call_ids)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![("aa".repeat(16), "77".repeat(8))],
        );
    }

    /// Withheld reasoning has no text; its exported signature is the part, so a carrier holding exactly that
    /// signature is its home, and one holding a different signature is not.
    #[test]
    fn withheld_reasoning_is_at_home_where_its_signature_is() {
        let (truth, _, _) = baseline(FIXTURE);
        let mut reasoning = truth.facts[0].clone();
        reasoning.kind = "reasoning".to_string();
        reasoning.value = serde_json::json!({"text": ""});
        // A seal is the SHA-256 of the signature, as truths record it.
        reasoning.seal = Some(crate::message_truth::truth::hex_digest(
            &<sha2::Sha256 as sha2::Digest>::digest(b"EqQBCkgIBRAB"),
        ));
        let carrier = |seal: &str, span: &str| Carrier {
            trace: Some("ee".repeat(16)),
            span: Some(span.to_string()),
            strings: vec![("span.attributes.signature".to_string(), seal.to_string())],
            ..Carrier::default()
        };
        let haystack = Haystack {
            carriers: vec![
                carrier("EqQBCkgIBRAB", &"11".repeat(8)),
                carrier("other", &"22".repeat(8)),
            ],
            undecoded: Vec::new(),
        };
        assert_eq!(
            carrying_spans(&reasoning, &haystack, &Default::default())
                .into_iter()
                .collect::<Vec<_>>(),
            vec![("ee".repeat(16), "11".repeat(8))],
        );
    }
}
