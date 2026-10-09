/// **A family index is spelled the way the convention writes it.** `msgs.01.role` beside `msgs.1.role` landed on
/// one entry and one member, so which value the message held followed the attribute map's hash order, and the
/// entry claimed `msgs.1.role` whichever key it had read. A segment with a sign or a leading zero is not an index:
/// its key is not read and stays unclaimed.
#[test]
fn a_family_index_with_a_leading_zero_or_a_sign_is_not_an_index() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.f","read":{"indexed_family":"msgs"},
            "require_members":{"all_of":[{"name":"role"}]},"emit":"message","priority":1}]}"#,
    )
    .expect("the probe compiles");
    let attrs = probe_attrs(&[
        ("msgs.1.role", "user"),
        ("msgs.1.content", "q"),
        ("msgs.01.role", "assistant"),
        ("msgs.+1.role", "assistant"),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emissions = plan.run(&ctx);
    assert_eq!(
        emissions
            .iter()
            .map(|e| e.value.clone())
            .collect::<Vec<_>>(),
        [serde_json::json!({"content": "q", "role": "user"})],
        "only the canonically indexed keys are an entry"
    );
    let owned: Vec<&str> = emissions
        .iter()
        .flat_map(|e| &e.owns)
        .map(|o| o.name.as_str())
        .collect();
    assert!(
        owned.contains(&"msgs.1.role")
            && !owned
                .iter()
                .any(|name| name.contains("01") || name.contains('+')),
        "the entry owns the keys it read, and only those: {owned:?}"
    );
}

/// **A covered disjunct is dead only where an unknown group is rejected.** Beneath a negation, a group that is
/// unknown rather than false leaves the negation unknown rather than true, so a disjunct true only where another
/// is still decides whether the rule applies - and refusing it refused a working condition.
#[test]
fn a_covered_disjunct_beneath_a_negation_is_live() {
    use crate::rules::span_conditions::{Readable, SpanSubject, holds, lower};
    let checked = |condition: serde_json::Value| {
        let condition: crate::rules::schema::SpanWhere =
            serde_json::from_value(condition).expect("the probe parses");
        crate::rules::detect_rules::checked_condition(&condition, Readable::SPAN)
    };
    let group = serde_json::json!({"any": [
        {"source": "attr:k", "exists": true},
        {"all": [{"source": "attr:k", "equals": "x"}, {"source": "attr:y", "exists": true}]},
    ]});
    assert!(
        checked(group.clone()).is_err(),
        "unnegated, the second disjunct can never be why the group held"
    );
    let negated = serde_json::json!({"not": group});
    assert!(
        checked(negated.clone()).is_ok(),
        "negated, it is live and the condition compiles"
    );
    // And it is live: with `k` absent and `y` present the condition withholds, and without the disjunct it holds.
    let lowered = |condition: serde_json::Value| {
        let condition: crate::rules::schema::SpanWhere =
            serde_json::from_value(condition).expect("the probe parses");
        lower(&condition, Readable::SPAN).expect("the probe lowers")
    };
    let attrs = probe_attrs(&[("y", "1")]);
    let subject = SpanSubject {
        span_name: "span",
        attrs: &attrs,
        scope_name: None,
        scope_version: None,
        resource: None,
    };
    assert!(!holds(&lowered(negated), &subject));
    assert!(holds(
        &lowered(serde_json::json!({"not": {"source": "attr:k", "exists": true}})),
        &subject
    ));
    // A repeat is dead in every one of the three values, so it is refused beneath a negation too.
    assert!(
        checked(serde_json::json!({"not": {"any": [
            {"source": "attr:k", "equals": "x"},
            {"source": "attr:k", "equals": "x"},
        ]}}))
        .is_err(),
        "a repeated disjunct is dead whatever the polarity"
    );
}

/// **A decoder and its fallback on one attribute are not contested.** A text the earlier rule's parse refuses is
/// one it reads nothing from and claims nothing of, so a later rule reading that text another way is live there.
/// Refused before, as though the JSON reading claimed every text.
#[test]
fn a_json_reading_does_not_suppress_a_text_reading_of_its_carrier() {
    use crate::rules::message_rules::{MessageCompileError, MessageContext};
    let pair = |first: &str, second: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[
                {{"id":"t.a","read":{{"attribute":"payload"}},"parse":"{first}","wrap":{{"role":"user"}},
                  "emit":"message","priority":1}},
                {{"id":"t.b","read":{{"attribute":"payload"}},"parse":"{second}","wrap":{{"role":"user"}},
                  "emit":"message","priority":2}}]}}"#
        ))
    };
    let plan = pair("json", "text").expect("a decoder and its text fallback compile");
    let read = |raw: &str| {
        let attrs = probe_attrs(&[("payload", raw)]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .into_iter()
            .map(|e| (e.rule_id.to_string(), e.value))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        read("hello"),
        [(
            "t.b".to_string(),
            serde_json::json!({"role": "user", "content": "hello"})
        )],
        "text that is not JSON is the fallback's"
    );
    assert_eq!(
        read(r#""hi""#),
        [(
            "t.a".to_string(),
            serde_json::json!({"role": "user", "content": "hi"})
        )],
        "and JSON is the decoder's alone"
    );
    // Where the earlier parse reads every text the later one does, the later rule is still dead.
    for (first, second) in [
        ("text", "json"),
        ("json", "json"),
        ("json", "stringified_array"),
    ] {
        assert!(
            matches!(
                pair(first, second),
                Err(MessageCompileError::ContestedCarrier { .. })
            ),
            "`{first}` before `{second}` leaves the second nothing to read"
        );
    }
}

/// **A declaration a family's branch never reaches is refused.** An indexed family returns before the element
/// passes and parses no carrier text, so an assistant-only pass beside it compiled while user entries still
/// emitted; and a `repr` grammar reads carrier text, which a dotted-prefix family does not have, so beside one it
/// compiled and could never emit.
#[test]
fn a_declaration_a_family_never_reaches_is_refused() {
    use crate::rules::message_rules::MessageCompileError;
    for (why, rule) in [
        (
            "element passes beside an indexed family",
            r#"{"id":"t.f","read":{"indexed_family":"msgs"},"emit":"message","priority":1,
                "elements":{"passes":[{"id":"assistant","where":{"path":"$.role","one_of":["assistant"]}}]}}"#,
        ),
        (
            "a rule-level parse beside an indexed family",
            r#"{"id":"t.f","read":{"indexed_family":"msgs"},"parse":"text","emit":"message","priority":1}"#,
        ),
        (
            "a repr grammar beside a dotted-prefix family",
            r#"{"id":"t.f","read":{"family":"tools."},"emit":"tool_definitions","priority":1,
                "tool_repr":{"name_field":"name","description_field":"description","name_label":"Name:",
                  "description_label":"Description:","arguments_label":"Arguments:","repr_markers":["Tool("],
                  "parameter_members":["parameters"],"type_map":[["str","string"]],
                  "type_default":"unconstrained","entries":"$","candidates":["$"]}}"#,
        ),
    ] {
        let refusal = probe_compile(&format!(r#"{{"id":"t","messages":[{rule}]}}"#));
        assert!(
            matches!(refusal, Err(MessageCompileError::Inexpressible { .. })),
            "{why}: {:?}",
            refusal.err()
        );
    }
}

/// **A swept literal is compared as Rust evaluates it.** Its spelling was compared, so an escape hid a framework
/// key from the attribute-key sweep - `"llm.cost.\u{74}otal"` is `llm.cost.total` to the compiler.
#[test]
fn an_escaped_literal_does_not_hide_a_framework_key_from_the_sweep() {
    assert_eq!(literal_text(r#""llm.cost.\u{74}otal""#), "llm.cost.total");
    assert_eq!(literal_text(r#""a\x2eb\n""#), "a.b\n");
    assert_eq!(
        literal_text(r##"r#"a\u{74}"#"##),
        r"a\u{74}",
        "a raw string has no escapes"
    );
    let keys =
        std::collections::BTreeMap::from([("llm.cost.total".to_string(), "probe".to_string())]);
    let offenders = attribute_key_offenders(
        "probe.rs",
        r#"fn read() -> &'static str { "llm.cost.\u{74}otal" }"#,
        &keys,
    );
    assert_eq!(
        offenders.len(),
        1,
        "the escaped key is found: {offenders:?}"
    );
}

/// **An overlay may decode a member of its parsed copy before selecting from it.** A JSON payload carrying the
/// provider's response as a Python `repr` in one string member holds the only ordered copy of the blocks; decoded,
/// it overlays the flattened family. A member that is absent or does not decode leaves the flattened form, and a
/// decode that could select nothing is refused.
#[test]
fn an_overlay_decodes_a_serialised_member_before_selecting() {
    use crate::rules::message_rules::{MessageCompileError, MessageContext};
    let rule = |decode: &str| {
        format!(
            r#"{{"id":"t","messages":[{{"id":"t.out","priority":1,"emit":"message",
                "read":{{"indexed_family":"fam","entry_member":"message",
                  "overlay":{{"from":"rich","parse":"json",{decode}"select":"$.choices",
                    "witness":{{"path":"$[*].message","exists":true}},
                    "when_member_prefix":"contents.","content_from":"$.message.content",
                    "where":{{"kind":"array"}},"as_member":"content"}}}},
                "require_members":{{"all_of":[{{"name":"role"}}]}}}}]}}"#
        )
    };
    let plan = probe_compile(&rule(
        r#""decode":{"select":"$.raw","parse":"python_constructor_repr"},"#,
    ))
    .expect("the probe compiles");
    let read = |rich: &str| {
        let attrs = probe_attrs(&[
            ("fam.0.message.role", "assistant"),
            ("fam.0.message.contents.0.message_content.text", "answer"),
            ("rich", rich),
        ]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .into_iter()
            .map(|e| e.value)
            .collect::<Vec<_>>()
    };
    let repr = r#"Response(choices=[Choice(message=Message(role='assistant', content=[{'type': 'thinking', 'thinking': 'hmm'}, {'type': 'text', 'text': 'answer'}]))])"#;
    let decoded = read(&serde_json::json!({"raw": repr}).to_string());
    assert_eq!(
        decoded[0]["content"],
        serde_json::json!([{"type": "thinking", "thinking": "hmm"}, {"type": "text", "text": "answer"}]),
        "the decoded member's blocks replace the flattened ones, in order: {decoded:?}"
    );
    for (why, rich) in [
        (
            "the member absent",
            serde_json::json!({"other": repr}).to_string(),
        ),
        (
            "the member not a repr",
            serde_json::json!({"raw": "not a repr"}).to_string(),
        ),
        (
            "the member not text",
            serde_json::json!({"raw": {"choices": []}}).to_string(),
        ),
    ] {
        let kept = read(&rich);
        assert!(
            kept[0].get("contents.0.message_content.text").is_some()
                && kept[0].get("content").is_none(),
            "{why}: the flattened form stays - {kept:?}"
        );
    }
    for decode in [
        r#""decode":{"select":"$","parse":"python_constructor_repr"},"#,
        r#""decode":{"select":"$.raw","parse":"text"},"#,
    ] {
        assert!(
            matches!(
                probe_compile(&rule(decode)),
                Err(MessageCompileError::Inexpressible { .. })
            ),
            "a decode that can select nothing is refused: {decode}"
        );
    }
}

/// **An overlay may put a counterpart's blocks before the flattened content and keep it.** A dialect that
/// flattens an answer but has no name for the reasoning that preceded it loses a *kind* of block, not the
/// content: replacing would drop the answer, so the serialised copy's blocks are prepended and the flattened
/// members renumbered after them. An empty list or an absent member leaves the flattened form standing.
#[test]
fn an_overlay_may_prepend_a_counterpart_s_blocks_and_keep_the_flattened_ones() {
    use crate::rules::message_rules::{MessageCompileError, MessageContext};
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.out","priority":1,"emit":"message",
            "read":{"indexed_family":"fam","entry_member":"message",
              "overlay":{"from":"rich","parse":"json","select":"$.choices",
                "witness":{"path":"$[*].message","exists":true},
                "when_member_prefix":"contents.","prepend_from":"$.message.thinking_blocks"}},
            "require_members":{"all_of":[{"name":"role"}]}}]}"#,
    )
    .expect("a prepending overlay compiles");
    let read = |rich: serde_json::Value| {
        let attrs = probe_attrs(&[
            ("fam.0.message.role", "assistant"),
            ("fam.0.message.contents.0.message_content.text", "answer"),
            ("fam.0.message.contents.1.message_content.text", "more"),
            ("rich", &rich.to_string()),
        ]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .into_iter()
            .map(|e| e.value)
            .collect::<Vec<_>>()
    };
    let thinking = serde_json::json!([
        {"type": "thinking", "thinking": "first", "signature": "sig-1"},
        {"type": "thinking", "thinking": "second", "signature": "sig-2"},
    ]);
    let entry =
        &read(serde_json::json!({"choices": [{"message": {"thinking_blocks": thinking}}]}))[0];
    assert_eq!(
        entry["contents.0"],
        serde_json::json!({"type": "thinking", "thinking": "first", "signature": "sig-1"})
    );
    assert_eq!(entry["contents.1"]["thinking"], serde_json::json!("second"));
    assert_eq!(
        entry["contents.2.message_content.text"],
        serde_json::json!("answer"),
        "the flattened content is kept, renumbered after the prepended blocks: {entry}"
    );
    assert_eq!(
        entry["contents.3.message_content.text"],
        serde_json::json!("more")
    );

    for (why, rich) in [
        (
            "an empty list",
            serde_json::json!({"choices": [{"message": {"thinking_blocks": []}}]}),
        ),
        (
            "an absent member",
            serde_json::json!({"choices": [{"message": {}}]}),
        ),
    ] {
        let kept = &read(rich)[0];
        assert_eq!(
            kept["contents.0.message_content.text"],
            serde_json::json!("answer"),
            "{why}: the flattened form stands - {kept}"
        );
        assert!(
            kept.get("contents.2.message_content.text").is_none(),
            "{why}"
        );
    }

    // A prepend-only overlay names no `as_member`, since it writes none.
    assert!(matches!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.out","priority":1,"emit":"message",
                "read":{"indexed_family":"fam","entry_member":"message",
                  "overlay":{"from":"rich","parse":"json","select":"$.choices",
                    "witness":{"path":"$[*].message","exists":true},
                    "when_member_prefix":"contents.","as_member":"content",
                    "prepend_from":"$.message.thinking_blocks"}},
                "require_members":{"all_of":[{"name":"role"}]}}]}"#
        ),
        Err(MessageCompileError::Inexpressible { .. })
    ));
}

/// **A prepending overlay renumbers every flattened block, however many there are.** The first version shifted
/// the members in place, highest first by *name*: `contents.9` sorts above `contents.10`, so it was moved onto it
/// before it was read and that block was lost. Ten or more blocks is where it bites, and no corpus answer has ten
/// yet - so the sizes are checked here, including past 100, where the lexicographic order misleads twice.
#[test]
fn a_prepending_overlay_keeps_every_flattened_block() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.out","priority":1,"emit":"message",
            "read":{"indexed_family":"fam","entry_member":"message",
              "overlay":{"from":"rich","parse":"json","select":"$.choices",
                "witness":{"path":"$[*].message","exists":true},
                "when_member_prefix":"contents.","prepend_from":"$.message.thinking_blocks"}},
            "require_members":{"all_of":[{"name":"role"}]}}]}"#,
    )
    .expect("the probe compiles");
    for blocks in [11usize, 12, 101] {
        let mut attrs: Vec<(String, String)> = vec![
            ("fam.0.message.role".to_string(), "assistant".to_string()),
            (
                "rich".to_string(),
                serde_json::json!({"choices": [{"message": {"thinking_blocks": [
                    {"type": "thinking", "thinking": "first", "signature": "sig"}
                ]}}]})
                .to_string(),
            ),
        ];
        for at in 0..blocks {
            attrs.push((
                format!("fam.0.message.contents.{at}.message_content.text"),
                format!("block {at}"),
            ));
        }
        let borrowed: Vec<(&str, &str)> = attrs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let probe = probe_attrs(&borrowed);
        let ctx = MessageContext::for_span("span", &probe, false);
        let emissions = plan.run(&ctx);
        let entry = &emissions[0].value;
        assert_eq!(
            entry["contents.0"]["thinking"],
            serde_json::json!("first"),
            "{blocks} blocks: the prepended block is first"
        );
        for at in 0..blocks {
            assert_eq!(
                entry[format!("contents.{}.message_content.text", at + 1)],
                serde_json::json!(format!("block {at}")),
                "{blocks} blocks: block {at} is kept, one place later"
            );
        }
        let kept = entry
            .as_object()
            .expect("an object")
            .keys()
            .filter(|member| member.starts_with("contents."))
            .count();
        assert_eq!(
            kept,
            blocks + 1,
            "{blocks} blocks: none lost, none invented"
        );
    }
}

/// **A member of a JSON attribute is tested in the value grammar, and the answer is three-valued and typed.**
///
/// The three answers compose with `parses`: unknown where the attribute is absent, false where it is present and
/// does not parse, and otherwise what the value grammar says of the selected member - a member that is not there
/// satisfies nothing. Typed is the point: `equals: true` asks about the flag, not about the four characters
/// `true`, which is what a substring of the payload's text would have matched.
#[test]
fn a_member_of_a_json_attribute_is_tested_typed_and_three_valued() {
    use super::expr::Truth::{False as F, True as T, Unknown as U};
    use super::span_conditions::{self, Readable, SpanSubject};
    let lower = |condition: serde_json::Value| {
        let parsed: super::schema::SpanWhere =
            serde_json::from_value(condition).expect("the probe condition parses");
        span_conditions::lower(&parsed, Readable::ATTRIBUTES)
    };
    let streaming = serde_json::json!({
        "source": "attr:request_data", "parses": "json",
        "member": {"path": "$.stream", "equals": true}
    });
    let positive = lower(streaming.clone()).expect("the test lowers");
    let negated = lower(serde_json::json!({"not": streaming})).expect("and its negation");
    let truth = |condition: &span_conditions::SpanExpr, value: Option<&str>| {
        let attrs: std::collections::HashMap<String, String> = value
            .map(|v| ("request_data".to_string(), v.to_string()))
            .into_iter()
            .collect();
        let subject = SpanSubject {
            span_name: "span",
            attrs: &attrs,
            scope_name: None,
            scope_version: None,
            resource: None,
        };
        condition.eval(&mut |atom| atom.eval(&subject))
    };
    for (why, value, is, is_not) in [
        ("the flag is set", Some(r#"{"stream": true}"#), T, F),
        ("the flag is clear", Some(r#"{"stream": false}"#), F, T),
        (
            "the flag is set, among others",
            Some(r#"{"model": "m", "stream": true, "temperature": 0}"#),
            T,
            F,
        ),
        // Typed, which is the whole reason this is not a substring test: the text `"true"` is not the flag, and
        // a payload that merely mentions the word is not a streamed request.
        (
            "the text, not the flag",
            Some(r#"{"stream": "true"}"#),
            F,
            T,
        ),
        // A member that is not there is **unknown**, as it is everywhere else in the value grammar: "no value
        // satisfied this" and "there was no value to ask about" are different answers, and an asset that wants
        // the second to be false writes `exists: true` beside the test, which is checked below.
        (
            "the word in another member",
            Some(r#"{"prompt": "answer with stream: true"}"#),
            U,
            U,
        ),
        ("no such member", Some(r#"{"model": "m"}"#), U, U),
        // A null member is a value, and it is not the flag.
        ("a null member", Some(r#"{"stream": null}"#), F, T),
        // Present and unreadable is false, as `parses` answers for the same text: the attribute is there and
        // does not hold what the condition asks about.
        ("text cut short", Some(r#"{"stream": tr"#), F, T),
        ("not JSON at all", Some("streaming"), F, T),
        // And absent is unknown, so a `not` over it does not hold for a span that carries no such attribute.
        ("no attribute", None, U, U),
    ] {
        assert_eq!(
            (truth(&positive, value), truth(&negated, value)),
            (is, is_not),
            "{why}: {value:?}"
        );
    }

    // The whole value grammar applies to the member, not just equality.
    let kinds = lower(serde_json::json!({
        "source": "attr:request_data", "parses": "json",
        "member": {"path": "$.messages", "kind": "array", "non_empty": true}
    }))
    .expect("a kind and emptiness test lowers");
    assert_eq!(
        truth(&kinds, Some(r#"{"messages": [{"role": "user"}]}"#)),
        T
    );
    assert_eq!(truth(&kinds, Some(r#"{"messages": []}"#)), F);
    assert_eq!(truth(&kinds, Some(r#"{"messages": "one"}"#)), F);

    // `exists` beside the test turns a member that is not there into a false answer, which is how the same
    // escape hatch works for an absent attribute in the span grammar.
    let required = lower(serde_json::json!({
        "source": "attr:request_data", "parses": "json",
        "member": {"path": "$.stream", "exists": true, "equals": true}
    }))
    .expect("a required member lowers");
    assert_eq!(truth(&required, Some(r#"{"stream": true}"#)), T);
    assert_eq!(truth(&required, Some(r#"{"model": "m"}"#)), F);
    assert_eq!(
        truth(&required, None),
        U,
        "an absent attribute stays unknown: the span said nothing about it"
    );

    // A member test says the attribute is there, so it implies the existence test.
    let exists = lower(serde_json::json!({"source": "attr:request_data", "exists": true}))
        .expect("an existence test lowers");
    assert!(span_conditions::implies(&positive, &exists));

    // Refusals. A member without the encoding that says how the text is read; a member of a source that has no
    // text to parse; and a `member` holding no condition, which would be `parses` written twice.
    for (why, condition) in [
        (
            "no encoding",
            serde_json::json!({"source": "attr:request_data",
                "member": {"path": "$.stream", "equals": true}}),
        ),
        (
            "a source with nothing to parse",
            serde_json::json!({"source": "span_name", "parses": "json",
                "member": {"path": "$.stream", "equals": true}}),
        ),
        (
            "no condition on the member",
            serde_json::json!({"source": "attr:request_data", "parses": "json", "member": {}}),
        ),
    ] {
        assert!(lower(condition).is_err(), "{why}: accepted");
    }
}

/// A version range is still asked alone: the member test is a test like any other beside it.
#[test]
fn a_version_range_is_not_asked_beside_a_member_test() {
    let parsed: super::schema::SpanWhere = serde_json::from_value(serde_json::json!({
        "source": "scope.version", "parses": "json",
        "member": {"path": "$.x", "exists": true},
        "version": {"scheme": "semver", "at_least": "1.0.0", "because": "a probe"}
    }))
    .expect("the probe condition parses");
    assert!(
        super::span_conditions::lower(&parsed, super::span_conditions::Readable::ALL).is_err(),
        "a version range beside a member test reads the version as text"
    );
}
