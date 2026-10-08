/// **A reading marks the messages that are renderings.** `rendering` is asked of the same value as the
/// reading's `where` - the candidate of an alternative, the entry of an indexed family - and only the messages
/// it holds for carry the mark; a fragment case's own declaration adds to its selection point's.
#[test]
fn a_reading_marks_its_renderings_and_nothing_else() {
    use crate::rules::message_rules::{MessageContext, compile};
    let plan = |rules: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rules}]}}"#);
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
        .expect("the probe compiles")
    };
    let marks = |plan: &super::message_rules::MessagePlan, attrs: &[(&str, &str)]| {
        let attrs: std::collections::HashMap<String, String> = attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .into_iter()
            .map(|e| {
                (
                    e.value["content"].as_str().unwrap_or_default().to_string(),
                    e.rendering,
                )
            })
            .collect::<Vec<_>>()
    };

    // Alternatives: one reading marks by role, an `also` reading marks nothing.
    let alternatives = plan(
        r#"{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message","priority":1,
            "alternatives":[{"id":"turns","select":"$.turns","each":true,"where":{"path":"$.role","exists":true},
              "rendering":{"path":"$.role","one_of":["tool-call"]},
              "wrap":{"role":"assistant","content_from":"$.text"}}],
            "also":[{"id":"answer","select":"$.answer","where":{"kind":"string"},"wrap":{"role":"assistant"}}]}"#,
    );
    assert_eq!(
        marks(
            &alternatives,
            &[(
                "x",
                r#"{"turns":[{"role":"user","text":"q"},{"role":"tool-call","text":"Calling tools"}],"answer":"a"}"#
            )]
        ),
        [
            ("q".to_string(), false),
            ("Calling tools".to_string(), true),
            ("a".to_string(), false)
        ]
    );

    // An indexed family: the entries the condition holds for, asked of the entry `entry_where` sees.
    let family = plan(
        r#"{"id":"t.f","read":{"indexed_family":"msgs","entry_member":"message",
            "rendering":{"path":"$.role","one_of":["tool-response"]}},"emit":"message","priority":1}"#,
    );
    assert_eq!(
        marks(
            &family,
            &[
                ("msgs.0.message.role", "user"),
                ("msgs.0.message.content", "q"),
                ("msgs.1.message.role", "tool-response"),
                ("msgs.1.message.content", "Observation: sunny"),
            ]
        ),
        [
            ("q".to_string(), false),
            ("Observation: sunny".to_string(), true)
        ]
    );

    // A fragment case's own declaration marks what its selection point did not.
    let fragments = |selection: &str, case: &str| {
        format!(
            r#"{{"id":"t","fragments":{{"shapes":{{"cases":[{{"id":"turn","where":{{"path":"$.role","exists":true}}{case},
                "wrap":{{"role":"assistant","content_from":"$.text"}}}}]}}}},
              "messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message","priority":1,
                "alternatives":[{{"id":"each","select":"$.turns","each":true,"then_fragment":"t.shapes"{selection}}}]}}]}}"#
        )
    };
    let compiled = |body: String| {
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
        .expect("the probe compiles")
    };
    let turns = [(
        "x",
        r#"{"turns":[{"role":"user","text":"q"},{"role":"tool-call","text":"r"}]}"#,
    )];
    let by_case = compiled(fragments(
        "",
        r#","rendering":{"path":"$.role","one_of":["tool-call"]}"#,
    ));
    assert_eq!(
        marks(&by_case, &turns),
        [("q".to_string(), false), ("r".to_string(), true)]
    );
    let by_selection = compiled(fragments(
        r#","rendering":{"path":"$.role","one_of":["user"]}"#,
        "",
    ));
    assert_eq!(
        marks(&by_selection, &turns),
        [("q".to_string(), true), ("r".to_string(), false)]
    );
}

/// **A marker that cannot take effect is refused.** Only a message is left out of a view; an aggregate's one
/// observation stands for all of its entries; and on a read, the marker belongs to an indexed family's entries.
#[test]
fn a_rendering_that_cannot_take_effect_is_refused() {
    use crate::rules::message_rules::compile;
    let refused = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
        .is_err()
    };
    let marked = r#""rendering":{"path":"$.role","one_of":["tool-call"]}"#;
    assert!(
        !refused(&format!(
            r#"{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message","priority":1,
                "alternatives":[{{"id":"a","select":"$.turns","each":true,{marked},"wrap":{{"role":"user","content_from":"$.text"}}}}]}}"#
        )),
        "a message reading may mark its renderings"
    );
    for (rule, why) in [
        (
            format!(
                r#"{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"tool_definitions","priority":1,
                    "alternatives":[{{"id":"a","select":"$.tools",{marked}}}]}}"#
            ),
            "a reading that emits tool definitions has no message to leave out",
        ),
        (
            format!(
                r#"{{"id":"t.r","read":{{"attribute":"x",{marked}}},"parse":"json","emit":"message","priority":1}}"#
            ),
            "a read that is not an indexed family has no entries to mark",
        ),
        (
            format!(
                r#"{{"id":"t.r","read":{{"indexed_family":"msgs",{marked}}},"emit":"message","priority":1,
                    "aggregate_into_array":true,"wrap":{{"role":"user"}}}}"#
            ),
            "an aggregate's one observation is not one entry",
        ),
    ] {
        assert!(refused(&rule), "{why}: {rule}");
    }
}
