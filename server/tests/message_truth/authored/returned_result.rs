//! A structured result a framework returns (`returns_arguments_of`): a framework text owed as JSON, explained only
//! as the holding span's own output, byte for byte the call's arguments.

use super::super::mutate::locate;
use super::super::mutations::{added, baseline};
use super::super::recon::ViewKind;

/// The result is shown once, as written, on the span that wrote it as its output: one byte off, on another span, or
/// as context sent to the span rather than its output, it is not the result.
#[test]
fn a_returned_result_is_owed_as_its_span_wrote_it() {
    let (truth, recon, baseline) = baseline("strands/sdk/structured_output");
    let result = truth
        .facts
        .iter()
        .find(|f| f.evidence == "framework")
        .expect("the returned result");
    let in_trace: Vec<(usize, usize)> = locate(result, &recon)
        .into_iter()
        .filter(|&(v, _)| recon.views[v].kind == ViewKind::Trace)
        .collect();
    let &(v, b) = in_trace.first().expect("the trace view shows it");
    assert!(
        recon.views[v].blocks[b].output,
        "shown as its span's output"
    );

    let mut altered = recon.clone();
    let city = altered.views[v].blocks[b].content["data"]["city"]
        .as_str()
        .expect("a city")
        .to_string();
    altered.views[v].blocks[b].content["data"]["city"] =
        format!("{}x", &city[..city.len() - 1]).into();
    altered.views[v].blocks[b].refresh();
    let new = added(&truth, &altered, &baseline);
    assert!(
        new.contains(&format!("text.missing:{}", result.id)),
        "the result one byte off passed: {new:?}"
    );

    let mut elsewhere = recon.clone();
    elsewhere.views[v].blocks[b].span = "a-span-that-did-not-write-it".into();
    let new = added(&truth, &elsewhere, &baseline);
    assert!(
        new.contains(&format!(
            "extra.unexplained:{}",
            elsewhere.views[v].blocks[b].digest
        )),
        "the result on another span passed: {new:?}"
    );

    let mut sent = recon.clone();
    sent.views[v].blocks[b].output = false;
    let new = added(&truth, &sent, &baseline);
    assert!(
        new.contains(&format!(
            "extra.unexplained:{}",
            sent.views[v].blocks[b].digest
        )),
        "the result as context sent to its span passed: {new:?}"
    );

    // The result where it belongs and a copy of it elsewhere: the copy is still a duplicate, on another span or
    // as context.
    for misplaced in ["span", "context"] {
        let mut copied = recon.clone();
        let mut copy = copied.views[v].blocks[b].clone();
        if misplaced == "span" {
            copy.span = "a-span-that-did-not-write-it".into();
        } else {
            copy.output = false;
        }
        copied.views[v].blocks.insert(b + 1, copy);
        let new = added(&truth, &copied, &baseline);
        assert!(
            !new.is_empty(),
            "a copy of the result as {misplaced} passed"
        );
    }

    // A model answer stating the same data, on the model call's own span: the framework's fact still takes only
    // the block its span wrote, and leaves the answer to the answer's fact.
    let mut answered = truth.clone();
    let call = answered.calls[0].id.clone();
    let mut answer = result.clone();
    answer.id = "fact-answer".into();
    answer.evidence = "wire".into();
    answer.call = Some(call.clone());
    answer.kept_from_model = false;
    answered.calls[0].outputs.push(answer.id.clone());
    let sequence = &mut answered.conversations[0].sequence;
    let at = sequence
        .iter()
        .position(|id| *id == result.id)
        .expect("the result's place");
    sequence.insert(at, answer.id.clone());
    answered.facts.push(answer);
    let mut both = recon.clone();
    let generation = &recon.generations[recon
        .generations
        .iter()
        .position(|g| g.typed && g.trace == recon.views[v].blocks[b].trace)
        .expect("the model call's span")];
    let mut model_block = both.views[v].blocks[b].clone();
    model_block.span = generation.span.clone();
    model_block.refresh();
    both.views[v].blocks.insert(b, model_block);
    let new = added(&answered, &both, &baseline);
    assert!(
        !new.iter().any(|a| a.ends_with(&format!(":{}", result.id))),
        "the framework's result took the model's answer: {new:?}"
    );
}

/// A framework's reply is held only where the span states it as its own output, as the readers read it: the bytes
/// beside the output in a diagnostic member, in an input event, or under an attribute no carrier holds as output hold
/// nothing.
#[test]
fn only_an_output_carrier_holds_a_reply() {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, KeyValueList, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, span::Event};
    use prost::Message;
    let (output, echoed) = (r#"{"city":"Vienna"}"#, r#"{"city":"Graz"}"#);
    let string = |key: &str, value: &str| KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
    };
    let span = |id: u8, attributes: Vec<KeyValue>, events: Vec<Event>| Span {
        trace_id: vec![1; 16],
        span_id: vec![id; 8],
        attributes,
        events,
        ..Default::default()
    };
    let choice = Event {
        name: "gen_ai.choice".into(),
        attributes: vec![string("message", output), string("diagnostic.echo", echoed)],
        ..Default::default()
    };
    // A choice whose content a reader takes over the message beside it.
    // An unset member states nothing: the reader takes the next.
    let unset = KeyValue {
        key: "content".into(),
        value: Some(AnyValue { value: None }),
    };
    let after_unset = r#"{"city":"Klagenfurt"}"#;
    let unset_first = Event {
        name: "gen_ai.choice".into(),
        attributes: vec![unset.clone(), string("message", after_unset)],
        ..Default::default()
    };
    let passed_over = Event {
        name: "gen_ai.choice".into(),
        attributes: vec![
            string("content", "a different reply"),
            string("message", echoed),
        ],
        ..Default::default()
    };
    let input = Event {
        name: "gen_ai.user.message".into(),
        attributes: vec![string("content", echoed)],
        ..Default::default()
    };
    // An output messages list: the assistant's part is content; a diagnostic member beside it, and a tool's
    // message in the same list, are not.
    let listed = r#"{"city":"Linz"}"#;
    let messages = serde_json::json!([
        {"role": "assistant", "parts": [{"type": "text", "content": listed}], "diagnostic": {"echo": echoed}},
        {"role": "tool", "parts": [{"type": "text", "content": echoed}]},
        {"role": "assistant", "parts": [{"type": "reasoning", "content": echoed}]}
    ])
    .to_string();
    // The deprecated spellings of a response are output carriers too.
    let deprecated = r#"{"city":"Innsbruck"}"#;
    let (enveloped, native) = (r#"{"city":"Bregenz"}"#, r#"{"city":"Villach"}"#);
    let spelled = r#"{"city":"Wels"}"#;
    let (unflagged, indexed) = (r#"{"city":"Steyr"}"#, r#"{"city":"Dornbirn"}"#);
    let bare = r#"{"city":"Leoben"}"#;
    let both_members = serde_json::json!([
        {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": enveloped}]},
        {"role": "assistant", "parts": [{"type": "text", "text": "a different reply", "content": echoed}]},
        {"role": "assistant", "content": "a different reply", "text": echoed},
        {"role": "assistant", "parts": [{"type": "text", "text": {"text": echoed}}]},
        {"role": "assistant", "parts": [{"reasoning": {"text": "thinking"}, "text": echoed}]},
        {"role": "model", "parts": [{"type": "text", "text": spelled}]},
        {"role": "assistant", "parts": [{"type": "text", "content": {"text": echoed}}]},
        {"role": "Assistant", "content": unflagged, "thought": null},
        {"role": "assistant", "parts": [{"reasoning_content": {}, "text": echoed}, {"reasoning": false, "text": echoed}]},
        {"role": "assistant", "parts": [{"type": "output_text", "text": [{"text": echoed}]}]},
        {"role": "tool_call", "name": "x", "content": echoed},
        {"role": "assistant", "content": [bare]}
    ])
    .to_string();
    // The same span id in another trace is another span.
    let elsewhere = r#"{"city":"Salzburg"}"#;
    let other_trace = Event {
        name: "gen_ai.choice".into(),
        attributes: vec![string("message", elsewhere)],
        ..Default::default()
    };
    let request = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![
                    span(1, Vec::new(), vec![choice]),
                    span(2, Vec::new(), vec![input]),
                    span(3, vec![string("input.value", echoed)], Vec::new()),
                    span(11, Vec::new(), vec![passed_over]),
                    span(
                        13,
                        vec![
                            string("gen_ai.completion.0.role", "Assistant"),
                            string("gen_ai.completion.0.content", indexed),
                        ],
                        Vec::new(),
                    ),
                    span(12, Vec::new(), vec![unset_first]),
                    // An indexed completion beside a tool's role.
                    span(
                        8,
                        vec![
                            string("gen_ai.completion.0.role", "tool"),
                            string("gen_ai.completion.0.content", echoed),
                        ],
                        Vec::new(),
                    ),
                    // A part's text is what it says; a content beside it is not; an envelope is no part.
                    span(
                        9,
                        vec![string("gen_ai.output.messages", &both_members)],
                        Vec::new(),
                    ),
                    // A choice written as a native OTLP map.
                    span(
                        10,
                        Vec::new(),
                        vec![Event {
                            name: "gen_ai.choice".into(),
                            attributes: vec![KeyValue {
                                key: "message".into(),
                                value: Some(AnyValue {
                                    value: Some(any_value::Value::KvlistValue(KeyValueList {
                                        values: vec![
                                            string("role", "assistant"),
                                            string("content", native),
                                        ],
                                    })),
                                }),
                            }],
                            ..Default::default()
                        }],
                    ),
                    span(
                        6,
                        vec![
                            string("gen_ai.completion.0.role", "assistant"),
                            string("gen_ai.completion.0.content", deprecated),
                        ],
                        Vec::new(),
                    ),
                    span(
                        7,
                        Vec::new(),
                        vec![Event {
                            name: "gen_ai.content.completion".into(),
                            attributes: vec![string("content", deprecated)],
                            ..Default::default()
                        }],
                    ),
                    span(
                        5,
                        vec![string("gen_ai.operation.name", "execute_tool")],
                        vec![Event {
                            name: "gen_ai.choice".into(),
                            attributes: vec![string("message", echoed)],
                            ..Default::default()
                        }],
                    ),
                    span(
                        4,
                        vec![string("gen_ai.output.messages", &messages)],
                        Vec::new(),
                    ),
                    Span {
                        trace_id: vec![2; 16],
                        ..span(1, Vec::new(), vec![other_trace])
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let dir = std::env::temp_dir().join(format!("authored-holders-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("req-001.pb");
    std::fs::write(&path, request.encode_to_vec()).expect("the request is written");
    let spans = super::raw_spans(std::slice::from_ref(&path));
    std::fs::remove_dir_all(&dir).ok();
    let held = |text: &str| -> Vec<(String, String)> {
        super::holders(&spans, text)
            .into_iter()
            .map(|(span, _)| span)
            .collect()
    };
    let at = |trace: u8, span: u8| {
        (
            super::hex_digest(&[trace; 16]),
            super::hex_digest(&[span; 8]),
        )
    };
    assert_eq!(
        held(listed),
        [at(1, 4)],
        "the assistant's part of an output list holds it"
    );
    assert_eq!(
        held(deprecated),
        [at(1, 6), at(1, 7)],
        "an indexed completion and the deprecated event hold it"
    );
    assert_eq!(
        held(enveloped),
        [at(1, 9)],
        "a message envelope's text holds it"
    );
    assert_eq!(
        held(spelled),
        [at(1, 9)],
        "the provider's own spelling of the assistant holds it"
    );
    assert_eq!(
        held(unflagged),
        [at(1, 9)],
        "a reasoning flag that is unset says nothing"
    );
    assert_eq!(
        held(indexed),
        [at(1, 13)],
        "an indexed completion's role compares as normalised"
    );
    assert_eq!(
        held(bare),
        [at(1, 9)],
        "a bare string in a message's content list holds it"
    );
    assert_eq!(
        held(native),
        [at(1, 10)],
        "a choice written as a native map holds it"
    );
    assert_eq!(
        held(after_unset),
        [at(1, 12)],
        "past an unset member, the next holds it"
    );
    assert_eq!(
        held(elsewhere),
        [at(2, 1)],
        "a span is its trace's: the same id elsewhere is another span"
    );
    assert_eq!(held(output), [at(1, 1)], "the choice's message holds it");
    assert!(
        held(echoed).is_empty(),
        "what the readers do not show as an assistant's output - a diagnostic member, a tool's message, an indexed completion of a tool's role, a reasoning part, a tool execution's choice, an input event and an input attribute among them - holds nothing"
    );
}
