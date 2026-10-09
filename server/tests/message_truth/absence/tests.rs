//! Adversarial cases: content the proof must find, in every location and under every encoding.

use base64::Engine as _;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use sha2::Digest as _;

use super::*;
use opentelemetry_proto::tonic::common::v1::{ArrayValue, KeyValueList};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{
    ResourceSpans, ScopeSpans, Span, Status, span::Event, span::Link,
};

const TEXT: &str = "Paris will be sunny on both days, so leave the umbrella at home.";
const CALL_ID: &str = "call_get_weather_dd57980a1fd3";

fn string(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.to_string())),
    }
}

fn kv(key: &str, value: AnyValue) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(value),
    }
}

fn fact(kind: &str, value: Value) -> Fact {
    serde_json::from_value(serde_json::json!({
        "id": "fact-001", "kind": kind, "role": "assistant", "conversation": "conv-1",
        "evidence": "wire", "value": value, "require": null
    }))
    .expect("a fact")
}

fn text_fact() -> Fact {
    fact("text", serde_json::json!({ "text": TEXT }))
}

fn call_fact() -> Fact {
    fact(
        "tool_call",
        serde_json::json!({"id": CALL_ID, "name": "get_weather", "arguments": {"city": "Paris", "days": 2}}),
    )
}

fn result_fact() -> Fact {
    fact(
        "tool_result",
        serde_json::json!({"call_id": CALL_ID, "name": "get_weather", "value": {"city": "Paris", "high_c": 21}, "is_error": false}),
    )
}

fn span(apply: impl FnOnce(&mut Span)) -> Haystack {
    let mut span = Span {
        name: "chat".to_string(),
        ..Default::default()
    };
    apply(&mut span);
    let request = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![span],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    Haystack::from_requests(&[("req-001.pb".to_string(), request)], &[])
}

fn attribute(value: AnyValue) -> Haystack {
    span(|s| s.attributes.push(kv("payload", value)))
}

fn log(apply: impl FnOnce(&mut LogRecord)) -> Haystack {
    let mut record = LogRecord::default();
    apply(&mut record);
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![record],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    Haystack::from_requests(&[], &[("logs-001.pb".to_string(), request)])
}

fn present(proof: Proof) -> bool {
    matches!(proof, Proof::Present(_))
}

#[test]
fn content_in_any_location_refuses_the_gap() {
    let fact = text_fact();
    let locations = [
        ("span attribute", attribute(string(TEXT))),
        ("span name", span(|s| s.name = TEXT.to_string())),
        (
            "status message",
            span(|s| {
                s.status = Some(Status {
                    message: TEXT.to_string(),
                    ..Default::default()
                })
            }),
        ),
        (
            "event attribute",
            span(|s| {
                s.events.push(Event {
                    name: "gen_ai.choice".to_string(),
                    attributes: vec![kv("message", string(TEXT))],
                    ..Default::default()
                })
            }),
        ),
        (
            "event name",
            span(|s| {
                s.events.push(Event {
                    name: TEXT.to_string(),
                    ..Default::default()
                })
            }),
        ),
        (
            "link attribute",
            span(|s| {
                s.links.push(Link {
                    attributes: vec![kv("note", string(TEXT))],
                    ..Default::default()
                })
            }),
        ),
        (
            "array member",
            attribute(AnyValue {
                value: Some(any_value::Value::ArrayValue(ArrayValue {
                    values: vec![string("x"), string(TEXT)],
                })),
            }),
        ),
        (
            "key-value list",
            attribute(AnyValue {
                value: Some(any_value::Value::KvlistValue(KeyValueList {
                    values: vec![kv("text", string(TEXT))],
                })),
            }),
        ),
        (
            "bytes",
            attribute(AnyValue {
                value: Some(any_value::Value::BytesValue(TEXT.as_bytes().to_vec())),
            }),
        ),
        ("log body", log(|r| r.body = Some(string(TEXT)))),
        (
            "log body key-value list",
            log(|r| {
                r.body = Some(AnyValue {
                    value: Some(any_value::Value::KvlistValue(KeyValueList {
                        values: vec![kv("content", string(TEXT))],
                    })),
                })
            }),
        ),
        (
            "log attribute",
            log(|r| r.attributes.push(kv("message", string(TEXT)))),
        ),
        ("log event name", log(|r| r.event_name = TEXT.to_string())),
        (
            "resource attribute",
            Haystack::from_requests(
                &[(
                    "req-001.pb".to_string(),
                    ExportTraceServiceRequest {
                        resource_spans: vec![ResourceSpans {
                            resource: Some(Resource {
                                attributes: vec![kv("service.note", string(TEXT))],
                                ..Default::default()
                            }),
                            ..Default::default()
                        }],
                    },
                )],
                &[],
            ),
        ),
    ];
    for (location, haystack) in locations {
        assert!(
            present(prove(&fact, &haystack)),
            "a text in a {location} was not found"
        );
    }
}

#[test]
fn content_under_any_encoding_refuses_the_gap() {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    let fact = text_fact();
    let json =
        serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": TEXT}]})
            .to_string();
    let encodings = [
        ("JSON in a string", json.clone()),
        (
            "JSON encoded twice",
            serde_json::to_string(&json).expect("a string"),
        ),
        (
            "a Python rendering",
            format!("{{'role': 'assistant', 'content': '{TEXT}'}}"),
        ),
        ("base64", STANDARD.encode(TEXT)),
        ("unpadded URL-safe base64", URL_SAFE_NO_PAD.encode(TEXT)),
        (
            "a JSON member holding base64",
            serde_json::json!({"data": STANDARD.encode(TEXT)}).to_string(),
        ),
        (
            "a base64 data URI",
            format!("data:text/plain;base64,{}", STANDARD.encode(TEXT)),
        ),
        (
            "a percent-encoded data URI",
            format!("data:text/plain,{}", TEXT.replace(' ', "%20")),
        ),
        ("backslash escapes", TEXT.replace(' ', "\\u0020")),
        ("reflowed whitespace", TEXT.replace(' ', "\n  ")),
    ];
    for (encoding, payload) in encodings {
        assert!(
            present(prove(&fact, &attribute(string(&payload)))),
            "a text under {encoding} was not found: {payload}"
        );
    }
}

#[test]
fn an_attachment_is_found_by_the_digest_of_its_bytes() {
    use base64::engine::general_purpose::STANDARD;
    let bytes = b"\xff\xd8\xff\xe0 not really a jpeg, but bytes all the same";
    let fact = fact(
        "user_media",
        serde_json::json!({"modality": "image", "media_type": "image/jpeg",
            "sha256": truth::hex_digest(&sha2::Sha256::digest(bytes)), "bytes": bytes.len()}),
    );
    let uri = format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes));
    let message =
        serde_json::json!({"content": [{"type": "image_url", "image_url": {"url": uri}}]});
    assert!(present(prove(
        &fact,
        &attribute(string(&message.to_string()))
    )));
    let raw = AnyValue {
        value: Some(any_value::Value::BytesValue(bytes.to_vec())),
    };
    assert!(present(prove(&fact, &attribute(raw))));
    // Bytes no script sends cannot be searched for by their encoding, so their absence is not proven.
    assert!(matches!(
        prove(&fact, &attribute(string("an image"))),
        Proof::Unprovable(_)
    ));
}

#[test]
fn an_attachment_sent_by_reference_is_found_by_the_place_it_names() {
    let url = "https://upload.example.com/commons/photo.jpg";
    let fact = fact(
        "user_media",
        serde_json::json!({"modality": "image", "media_type": null, "source": "url", "reference": url}),
    );
    let message = serde_json::json!({"parts": [{"type": "uri", "modality": "image", "uri": url}]});
    assert!(present(prove(
        &fact,
        &attribute(string(&message.to_string()))
    )));
    assert_eq!(
        prove(&fact, &attribute(string("an image the user linked"))),
        Proof::Absent
    );
    // Another place on the same host, or a longer id that begins with this one, is not this attachment.
    assert_eq!(
        prove(
            &fact,
            &attribute(string("https://upload.example.com/commons/other.jpg"))
        ),
        Proof::Absent
    );
    let file = fact_of_reference("file-1234567890");
    assert_eq!(
        prove(&file, &attribute(string("file-12345678901"))),
        Proof::Absent
    );
    assert!(present(prove(
        &file,
        &attribute(string("UriPart(file_id='file-1234567890')"))
    )));
    // A URL that goes on past the reference is another place; one a sentence closes on is this one.
    for other in [".backup", "/other.jpg", "?revision=2", "#part"] {
        assert_eq!(
            prove(&fact, &attribute(string(&format!("{url}{other}")))),
            Proof::Absent,
            "{url}{other}"
        );
    }
    assert!(present(prove(
        &fact,
        &attribute(string(&format!("The user linked {url}.")))
    )));
}

fn fact_of_reference(reference: &str) -> Fact {
    fact(
        "user_media",
        serde_json::json!({"modality": "file", "media_type": null, "source": "file_id", "reference": reference}),
    )
}

/// A script's attachment written inside other text - a Java `toString()` naming its base64 - is never
/// decoded as a value of its own, and is partly present all the same: no gap may claim it absent.
#[test]
fn an_attachment_written_inside_other_text_is_partial() {
    use base64::engine::general_purpose::STANDARD;
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/assets/img.jpg"),
    )
    .expect("the script's image");
    let fact = fact(
        "user_media",
        serde_json::json!({"modality": "image", "media_type": "image/jpeg",
            "sha256": truth::hex_digest(&sha2::Sha256::digest(&bytes)), "bytes": bytes.len()}),
    );
    let rendered = format!(
        "UserMessage {{ contents = [ImageContent {{ image = Image {{ base64Data = \"{}\" }} }}] }}",
        STANDARD.encode(&bytes)
    );
    assert!(matches!(
        prove(&fact, &attribute(string(&rendered))),
        Proof::Partial(_)
    ));
    // Cut short of its beginning, and wrapped into lines, it is still found.
    let encoded = STANDARD.encode(&bytes);
    let tail: String = encoded.as_bytes()[encoded.len() / 3..]
        .chunks(76)
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(matches!(
        prove(&fact, &attribute(string(&format!("image: {tail}")))),
        Proof::Partial(_)
    ));
    assert_eq!(
        prove(
            &fact,
            &attribute(string("Base64 string (218624 characters)"))
        ),
        Proof::Absent
    );
}

#[test]
fn a_tool_call_needs_its_witness_and_its_arguments_alone_are_partial() {
    let fact = call_fact();
    let call = serde_json::json!({"id": CALL_ID, "type": "function",
        "function": {"name": "get_weather", "arguments": "{\"city\": \"Paris\", \"days\": 2.0}"}});
    assert!(present(prove(&fact, &attribute(string(&call.to_string())))));
    let python = "{'name': 'get_weather', 'args': {'city': 'Paris', 'days': 2}}";
    assert!(
        present(prove(&fact, &attribute(string(python)))),
        "a Python rendering of the call"
    );
    // The id alone is what its result carries, so it proves nothing either way.
    let result = serde_json::json!({"role": "tool", "tool_call_id": CALL_ID, "content": "sunny"});
    assert_eq!(
        prove(&fact, &attribute(string(&result.to_string()))),
        Proof::Absent
    );
    let arguments = serde_json::json!({"days": 2, "city": "Paris"});
    assert!(matches!(
        prove(&fact, &attribute(string(&arguments.to_string()))),
        Proof::Partial(_)
    ));
    // Arguments as a key-value list under the call's name are the call.
    let listed = span(|s| {
        s.attributes
            .push(kv("gen_ai.tool.name", string("travel-get_weather")));
        s.attributes.push(kv(
            "gen_ai.tool.call.arguments",
            AnyValue {
                value: Some(any_value::Value::KvlistValue(KeyValueList {
                    values: vec![
                        kv("city", string("Paris")),
                        kv(
                            "days",
                            AnyValue {
                                value: Some(any_value::Value::IntValue(2)),
                            },
                        ),
                    ],
                })),
            },
        ));
    });
    assert!(present(prove(&fact, &listed)));
}

#[test]
fn a_tool_result_needs_its_call_id_and_its_value_alone_is_partial() {
    let fact = result_fact();
    let message = serde_json::json!({"role": "tool", "tool_call_id": CALL_ID,
        "content": "{\"result\": {\"high_c\": 21.0, \"city\": \"Paris\"}}"});
    assert!(present(prove(
        &fact,
        &attribute(string(&message.to_string()))
    )));
    let bare = serde_json::json!({"city": "Paris", "high_c": 21});
    assert!(matches!(
        prove(&fact, &attribute(string(&bare.to_string()))),
        Proof::Partial(_)
    ));
    assert_eq!(
        prove(&fact, &attribute(string(CALL_ID))),
        Proof::Absent,
        "the call id alone is the call's"
    );
    let short = fact_with_value(serde_json::json!(4));
    assert!(matches!(
        prove(&short, &attribute(string("nothing"))),
        Proof::Unprovable(_)
    ));
}

fn fact_with_value(value: Value) -> Fact {
    fact(
        "tool_result",
        serde_json::json!({"call_id": CALL_ID, "name": "calculate", "value": value, "is_error": false}),
    )
}

#[test]
fn a_truncated_text_is_partial_and_a_short_one_unprovable() {
    let fact = text_fact();
    let truncated: String = TEXT.chars().take(40).collect();
    assert!(matches!(
        prove(&fact, &attribute(string(&format!("{truncated}...")))),
        Proof::Partial(_)
    ));
    assert_eq!(
        prove(&fact, &attribute(string("an unrelated answer about Tokyo"))),
        Proof::Absent
    );
    let short = fact_with_text("Yes.");
    assert!(matches!(
        prove(&short, &attribute(string("nothing"))),
        Proof::Unprovable(_)
    ));
}

#[test]
fn a_text_is_truncated_only_where_a_payload_ends_on_its_prefix() {
    let cut: String = TEXT.chars().take(40).collect();
    // A preview the producer cut: the payload ends where the text was cut.
    assert!(
        super::truncated_at(TEXT, &attribute(string(&format!("Preamble.\n\n{cut}")))).is_some()
    );
    // A framework quoting the text in part and writing on: not a cut, so not a truncation.
    assert!(
        super::truncated_at(
            TEXT,
            &attribute(string(&format!("{cut} and then something else")))
        )
        .is_none()
    );
    // Held whole somewhere: nothing was cut.
    assert!(super::truncated_at(TEXT, &attribute(string(TEXT))).is_none());
    // Split across two exported strings, the rest in the second: all of it is carried.
    let rest: String = TEXT.chars().skip(40).collect();
    let split = span(|s| {
        s.attributes = vec![kv("first", string(&cut)), kv("second", string(&rest))];
    });
    assert!(super::truncated_at(TEXT, &split).is_none());
    // Split across three, the rest in two pieces neither of which holds its head: still all carried.
    let three = span(|s| {
        s.attributes = vec![
            kv("first", string("Paris will be sunny on both days,")),
            kv("second", string("so leave the")),
            kv("third", string("umbrella at home.")),
        ];
    });
    assert!(super::truncated_at(TEXT, &three).is_none());
}

#[test]
fn a_text_is_merged_only_where_one_carrier_holds_its_two_parts_whole() {
    let (first, rest) = TEXT.split_at(33);
    let parts = |a: &str, b: &str| {
        span(|s| {
            s.attributes = vec![kv("message.0", string(a)), kv("message.1", string(b))];
        })
    };
    assert!(super::merged_from(TEXT, &parts(first, rest)));
    // The second part only inside a longer string: a quotation, not a message.
    assert!(!super::merged_from(
        TEXT,
        &parts(first, &format!("{rest} And more."))
    ));
    // Held whole: nothing was merged.
    assert!(!super::merged_from(TEXT, &parts(TEXT, rest)));
}

fn fact_with_text(text: &str) -> Fact {
    fact("text", serde_json::json!({ "text": text }))
}

#[test]
fn an_encoding_deeper_than_the_search_is_unprovable() {
    let mut payload = serde_json::json!({"text": TEXT}).to_string();
    for _ in 0..MAX_DEPTH + 2 {
        payload = serde_json::to_string(&payload).expect("a string");
    }
    assert!(matches!(
        prove(&text_fact(), &attribute(string(&payload))),
        Proof::Unprovable(_)
    ));
}

/// End to end on a committed capture: a gap declaring a fact the fixture shows is refused.
#[test]
fn a_gap_on_a_fact_the_capture_shows_is_refused() {
    let truths = truth::load_all();
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let truth = &truths["strands/tool_use"];
    let fixture = truth
        .fixtures
        .iter()
        .find(|f| fixtures.contains_key(*f))
        .expect("a strands tool_use capture");
    let haystack = Haystack::of_fixture(&fixtures[fixture]);
    for kind in ["user_text", "text", "tool_call", "tool_result"] {
        let fact = truth
            .facts
            .iter()
            .find(|f| f.kind == kind && f.require.is_some())
            .unwrap_or_else(|| panic!("strands/tool_use has a {kind} fact"));
        assert!(
            present(prove(fact, &haystack)),
            "{fixture} shows {} but the proof did not find it",
            fact.id
        );
    }
}

/// A response with one call is still present when a message holds its arguments, whatever became of
/// the id; an execution span holding only the arguments does not make it so.
#[test]
fn a_response_is_present_when_a_message_holds_its_call() {
    let fact = call_fact();
    let response = Claim::Response(vec![&fact]);
    let message = serde_json::json!([{"role": "assistant", "parts": [
        {"type": "tool_call", "name": "get_weather", "arguments": {"city": "Paris", "days": 2}}]}]);
    assert!(present(prove_claim(
        &response,
        &attribute(string(&message.to_string()))
    )));
    let execution = span(|s| {
        s.attributes
            .push(kv("gen_ai.tool.name", string("get_weather")));
        s.attributes.push(kv(
            "gen_ai.tool.call.arguments",
            string(r#"{"city": "Paris", "days": 2}"#),
        ));
    });
    assert_eq!(prove_claim(&response, &execution), Proof::Absent);
}

/// The model's id for a call is absent, but the span executing it names the call under another: the
/// call could be shown under that one, so its id cannot be declared unexported.
#[test]
fn an_id_is_not_unexported_where_the_call_carries_another() {
    let fact = call_fact();
    let execution = |with_id: bool| {
        span(|s| {
            s.attributes
                .push(kv("gen_ai.tool.name", string("get_weather")));
            s.attributes.push(kv(
                "gen_ai.tool.call.arguments",
                string(r#"{"city": "Paris", "days": 2}"#),
            ));
            if with_id {
                s.attributes
                    .push(kv("gen_ai.tool.call.id", string("framework-7")));
            }
        })
    };
    assert!(matches!(
        prove_claim(&Claim::Id(&fact), &execution(true)),
        Proof::Partial(_)
    ));
    assert_eq!(
        prove_claim(&Claim::Id(&fact), &execution(false)),
        Proof::Absent
    );
}

/// A call's model is absent only where no payload contains it; its finish only where no member naming a
/// finish holds it - the same word in a sentence or a block type is not a finish stated.
#[test]
fn metadata_is_absent_only_where_no_payload_states_it() {
    let model = Claim::Metadata("model", vec!["global.anthropic.claude".to_string()]);
    assert!(present(prove_claim(
        &model,
        &attribute(string("bedrock/global.anthropic.claude"))
    )));
    assert_eq!(
        prove_claim(&model, &attribute(string("anthropic.claude"))),
        Proof::Absent
    );
    let finish = Claim::Metadata("finish", vec!["tool_use".to_string()]);
    let stated = span(|s| {
        s.attributes
            .push(kv("gen_ai.response.finish_reasons", string("TOOL_USE")));
    });
    assert!(present(prove_claim(&finish, &stated)));
    let block = serde_json::json!({"content": [{"type": "tool_use", "name": "get_weather"}]});
    assert_eq!(
        prove_claim(&finish, &attribute(string(&block.to_string()))),
        Proof::Absent
    );
}

fn withheld_fact() -> Fact {
    serde_json::from_value(serde_json::json!({
        "id": "fact-001", "kind": "reasoning", "role": "assistant", "conversation": "conv-1",
        "evidence": "wire", "value": {"text": "", "signed": true},
        "require": {"anchor": "model_call", "views": ["span"], "cardinality": "exactly_once", "match": "signed"}
    }))
    .expect("a fact")
}

#[test]
fn withheld_reasoning_is_absent_only_where_no_payload_holds_a_reasoning_part() {
    const SIGNATURE: &str = "EqQBCkgIBhABGAIiQKmvNk3zF7Yh0w2xQ5kVd9pLr8c3Tg1uB4oWnXeZsA6y";
    let fact = withheld_fact();
    let typed = serde_json::json!([{"type": "thinking", "thinking": "", "signature": SIGNATURE}]);
    assert!(present(prove(
        &fact,
        &attribute(string(&typed.to_string()))
    )));
    let signed = serde_json::json!({"reasoningContent": {"reasoningText": {"text": "", "signature": SIGNATURE}}});
    assert!(present(prove(
        &fact,
        &attribute(string(&signed.to_string()))
    )));
    let flagged = serde_json::json!({"parts": [{"thought": true, "text": ""}, {"text": TEXT}]});
    assert!(present(prove(
        &fact,
        &attribute(string(&flagged.to_string()))
    )));
    let flattened = span(|s| {
        s.attributes.push(kv(
            "gen_ai.output.messages.0.parts.0.type",
            string("reasoning"),
        ));
    });
    assert!(present(prove(&fact, &flattened)));

    // Configuration named for reasoning, and a signature that is no token, are not a part: a payload holding
    // only those exported no reasoning at all.
    let configured = serde_json::json!({
        "thinking": {"type": "enabled", "budget_tokens": 1024},
        "thinking_config": {"include_thoughts": true},
        "tools": [{"name": "get_weather", "signature": "(city: str, days: int) -> dict"}],
        "text": TEXT,
    });
    assert_eq!(
        prove(&fact, &attribute(string(&configured.to_string()))),
        Proof::Absent
    );
    // Withdrawn by a gap, the fact is proven by its value, not by the requirement it no longer has.
    let mut withdrawn = withheld_fact();
    withdrawn.require = None;
    assert!(present(prove(
        &withdrawn,
        &attribute(string(&typed.to_string()))
    )));
}

fn sealed_fact(token: &str) -> Fact {
    let mut fact = withheld_fact();
    fact.seal = Some(truth::hex_digest(&sha2::Sha256::digest(token.as_bytes())));
    fact
}

#[test]
fn a_known_seal_finds_its_signature_whatever_it_looks_like() {
    // Short and dotted: no opaque token by its looks, but the one the fact is sealed with.
    const OWN: &str = "sig.turn-1";
    let fact = sealed_fact(OWN);
    let part = serde_json::json!([{"type": "thinking", "thinking": "", "signature": OWN}]);
    let payload = attribute(string(&part.to_string()));
    assert!(present(prove_claim(&Claim::Signature(&fact), &payload)));
    assert!(present(prove(&fact, &payload)));
    let flattened = span(|s| {
        s.attributes
            .push(kv("gen_ai.thinking.signature", string(OWN)))
    });
    assert!(present(prove_claim(&Claim::Signature(&fact), &flattened)));
    // Without a seal the same short value is no token, and proves nothing.
    let mut unsealed = withheld_fact();
    unsealed.require = None;
    assert_eq!(
        prove_claim(&Claim::Signature(&unsealed), &flattened),
        Proof::Absent
    );
}

#[test]
fn a_decoded_signature_keeps_the_member_it_was_decoded_from() {
    const OWN: &str = "EqQBCkgIBhABGAIiQKmvNk3zF7Yh0w2xQ5kVd9pLr8c3Tg1uB4oWnXeZsA6y";
    const OTHER: &str = "ErUBCkYIBhABGAIiQGq1Lr7mWc2Vb8xNn3pKd0tTz5hJ9yRa4oUe6iSfX1gB";
    let fact = sealed_fact(OWN);
    // Each value is a JSON string literal, so the token itself is only in its decoded copy, at
    // `...signature|json`.
    let quoted = |token: &str| string(&serde_json::Value::String(token.to_string()).to_string());
    let flattened = |token: &str| {
        let token = token.to_string();
        span(move |s| {
            s.attributes.push(kv(
                "gen_ai.output.messages.0.parts.0.type",
                string("reasoning"),
            ));
            s.attributes.push(kv(
                "gen_ai.output.messages.0.parts.0.signature",
                quoted(&token),
            ));
        })
    };
    assert!(present(prove_claim(
        &Claim::Signature(&fact),
        &flattened(OWN)
    )));
    assert!(present(prove(&fact, &flattened(OWN))));
    // A part sealed with another turn's signature is not this one, decoded or not.
    assert_eq!(
        prove_claim(&Claim::Signature(&fact), &flattened(OTHER)),
        Proof::Absent
    );
    assert_eq!(prove(&fact, &flattened(OTHER)), Proof::Absent);
}

#[test]
fn why_a_response_ended_does_not_mark_its_text_as_reasoning() {
    const THOUGHT: &str = "The two slowest should cross together so the 10 absorbs the 5.";
    let fact = fact("reasoning", serde_json::json!({ "text": THOUGHT }));
    let kind = |payload: serde_json::Value| {
        prove_claim(
            &Claim::Kind(&fact),
            &attribute(string(&payload.to_string())),
        )
    };
    // A message stating why it ended carries a reason, not reasoning: its text is unmarked.
    assert_eq!(
        kind(
            serde_json::json!([{"role": "assistant", "finish_reason": "stop",
            "parts": [{"type": "text", "content": THOUGHT}]}])
        ),
        Proof::Absent
    );
    assert_eq!(
        kind(serde_json::json!({"stopReason": "end_turn", "content": [{"text": THOUGHT}]})),
        Proof::Absent
    );
    // A part typed as reasoning, or a member named for it, marks it.
    assert!(present(kind(
        serde_json::json!({"parts": [{"type": "reasoning", "content": THOUGHT}]})
    )));
    assert!(present(kind(
        serde_json::json!({"finish_reason": "stop", "reasoning_content": THOUGHT})
    )));
}
