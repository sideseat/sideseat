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
