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

    // An indexed family: the entries the condition holds for, asked of each assembled entry.
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

/// **The key sweeps still see a framework's key.** The grammar's own enumerated words (`scope.name`,
/// `scope.version`) are exempt from both, and nothing else is: a key one framework's asset declares, written in a
/// production module, trips each sweep, while the grammar words beside it do not.
#[test]
fn a_framework_key_in_production_trips_both_sweeps_and_a_grammar_word_does_not() {
    let telemetry = producer_key_inventory();
    let exclusive: Vec<(&String, &String)> = telemetry.iter().collect();
    let attribute = framework_attribute_keys();
    let key = attribute
        .keys()
        .find(|key| telemetry.contains_key(*key))
        .expect("a key both inventories hold");
    let module = format!(
        "fn read(attrs: &Attrs) {{\n    let a = attrs.get(\"{key}\");\n    let b = \"scope.version\";\n    let c = \"scope.name\";\n}}\n"
    );
    let telemetry_hits =
        telemetry_key_offenders("server/crates/domain/src/probe.rs", &module, &exclusive);
    let attribute_hits =
        attribute_key_offenders("server/crates/domain/src/probe.rs", &module, &attribute);
    for hits in [&telemetry_hits, &attribute_hits] {
        assert_eq!(
            hits.len(),
            1,
            "exactly the framework key is reported: {hits:?}"
        );
        assert!(hits[0].contains(key.as_str()), "{hits:?}");
    }
    let grammar = schema_grammar_words();
    let dotted: Vec<&String> = grammar.iter().filter(|word| word.contains('.')).collect();
    assert_eq!(
        dotted,
        ["scope.name", "scope.version"],
        "the dotted words the schema enumerates are the two source names, and nothing a producer writes"
    );
}

/// **A package name in code is still a framework's name.** The producer-word sweep reads an `observed_in`
/// range's `package` as provenance, and only there; every package an annotation names, written as a literal in a
/// production module, trips the framework-name sweep's markers.
#[test]
fn a_package_an_annotation_names_is_a_framework_name_in_code() {
    let parsed =
        ParsedAssets::parse(&crate::rules::schema::embedded_sources()).expect("the assets parse");
    let packages: std::collections::BTreeSet<&str> = parsed
        .observed()
        .iter()
        .flat_map(|leaf| leaf.ranges.iter().map(|range| range.package.as_str()))
        .collect();
    assert!(!packages.is_empty(), "the corpus annotates a clause");
    let markers = framework_markers();
    for package in packages {
        let module = format!("fn probe() -> &'static str {{\n    \"{package}\"\n}}\n");
        let named = production_names(&module, "server/crates/domain/src/probe.rs")
            .into_iter()
            .any(|(_, text)| {
                markers
                    .values()
                    .flatten()
                    .any(|marker| names_as_a_word(&text, marker))
            });
        assert!(
            named,
            "`{package}` in a production module is not caught as a framework's name"
        );
    }
}

/// **A defect in a prose-reporting section names where it is.** The sections that refused with a bare string -
/// message events, event roles, role authority, event categories, synthetic call ids, provider aliases, log
/// events and message projections - now name the clauses and the asset, so the diagnostic locates the
/// declaration rather than leaving a reader to search the corpus for the words.
#[test]
fn a_prose_section_defect_names_its_clause_and_asset() {
    let sources = std::collections::BTreeMap::from([(
        "producers/probe.json".to_string(),
        br#"{"id": "probe", "message_events": [{"id": "probe.event", "name": "probe.message"}],
             "event_roles": [{"id": "probe.role", "name": "probe.message", "role": "narrator"}]}"#
            .to_vec(),
    )]);
    let error =
        crate::rules::Ruleset::build(&ParsedAssets::parse(&sources).expect("the probe parses"))
            .err()
            .expect("a role that is not a role is refused");
    let diagnostic = error
        .diagnostics
        .iter()
        .find(|d| d.section.key() == "event_roles")
        .expect("the event-role section reports");
    assert!(
        diagnostic
            .locations
            .iter()
            .any(|location| location.clause.as_deref() == Some("probe.role")),
        "the clause is named: {diagnostic}"
    );
    assert!(
        diagnostic
            .locations
            .iter()
            .any(|location| location.asset_path.as_deref() == Some("producers/probe.json")),
        "and the asset that declares it: {diagnostic}"
    );
}

/// **A contradiction is a defect only where it is asserted.** Under a `not` it is a condition - one that holds
/// wherever the negated predicate answers false - so a condition of any shape has each atom judged at its
/// polarity. An ignored condition, and a root tested for presence alone, stay defects at either polarity: the
/// first is ignored under a `not` too, and the second is a tautology negated or not.
#[test]
fn a_predicate_is_judged_at_the_polarity_it_sits_at() {
    use crate::rules::message_rules::predicate_defect;
    use crate::rules::schema::ValueCondition;
    let defect = |value: serde_json::Value| {
        let condition: ValueCondition =
            serde_json::from_value(value.clone()).expect("the probe condition parses");
        predicate_defect(&condition)
    };
    let contradiction = serde_json::json!({"path": "$.x", "kind": "null", "not_null": true});
    assert!(
        defect(contradiction.clone()).is_some(),
        "asserted, it holds for nothing"
    );
    for (why, value) in [
        (
            "negated, it holds whenever the member exists",
            serde_json::json!({"not": contradiction.clone()}),
        ),
        (
            "and the same inside a conjunction under the negation",
            serde_json::json!({"not": {"all": [contradiction.clone(), {"path": "$.y"}]}}),
        ),
    ] {
        assert!(defect(value.clone()).is_none(), "{why}: refused - {value}");
    }
    for (why, value) in [
        (
            "two negations assert it again",
            serde_json::json!({"not": {"not": contradiction.clone()}}),
        ),
        (
            "a root tested for presence alone, negated",
            serde_json::json!({"not": {"path": "$"}}),
        ),
        (
            "a condition beside `exists: false`, ignored under a negation too",
            serde_json::json!({"not": {"path": "$.v", "exists": false, "kind": "string"}}),
        ),
    ] {
        assert!(defect(value.clone()).is_some(), "{why}: accepted - {value}");
    }
}

/// **An attribute answers `starts_with`**, three-valued like every value test: true where its value begins with
/// the prefix, false where it does not, and unknown where the attribute is absent, so a negation does not hold
/// there. An empty prefix is refused, and the analysis knows what it implies and excludes.
#[test]
fn an_attribute_answers_starts_with_in_three_values() {
    use crate::rules::expr::Truth;
    use crate::rules::span_conditions::{self, Readable, SpanAtom, SpanSubject};
    let lowered = |condition: serde_json::Value| {
        let condition: crate::rules::schema::SpanWhere =
            serde_json::from_value(condition).expect("the condition parses");
        span_conditions::lower(&condition, Readable::SPAN_AND_SCOPE)
    };
    let prefix =
        lowered(serde_json::json!({"source": "attr:probe.delta", "starts_with": "[USER]"}))
            .expect("an attribute answers starts_with");
    let truth = |value: Option<&str>| {
        let attrs: std::collections::HashMap<String, String> = value
            .map(|text| ("probe.delta".to_string(), text.to_string()))
            .into_iter()
            .collect();
        let subject = SpanSubject {
            span_name: "probe",
            attrs: &attrs,
            scope_name: None,
            scope_version: None,
            resource: None,
            marks: 0,
        };
        prefix.eval(&mut |atom| atom.eval(&subject))
    };
    assert_eq!(truth(Some("[USER]\nhello")), Truth::True);
    assert_eq!(
        truth(Some("[TOOL RESULT: t1]\n[USER] quoted")),
        Truth::False,
        "only where it begins"
    );
    assert_eq!(
        truth(None),
        Truth::Unknown,
        "absent is unknown, so `not` over it does not hold"
    );

    let refused = lowered(serde_json::json!({"source": "attr:probe.delta", "starts_with": ""}))
        .map(|lowered| {
            span_conditions::positive_atoms(&lowered)
                .iter()
                .any(|atom| atom.defect().is_some())
        });
    assert_eq!(refused.ok(), Some(true), "an empty prefix is a defect");

    let atom = |prefix: &str| SpanAtom::SpanAttrStartsWith {
        key: "probe.delta".to_string(),
        prefix: prefix.to_string(),
    };
    let equals = |value: &str| SpanAtom::SpanAttrEquals {
        key: "probe.delta".to_string(),
        value: value.to_string(),
    };
    assert!(
        atom("[USER]\n").implies(&atom("[USER]")),
        "a longer prefix implies a shorter"
    );
    assert!(equals("[USER]\nhi").implies(&atom("[USER]")));
    assert!(atom("[USER]").implies(&SpanAtom::SpanAttrExists {
        key: "probe.delta".to_string()
    }));
    assert!(!atom("[USER]").implies(&atom("[USER]\n")));
    assert!(
        atom("[USER]").excludes(&atom("[TOOL")),
        "two prefixes neither of which begins the other"
    );
    assert!(equals("[TOOL RESULT]").excludes(&atom("[USER]")));
    assert!(!atom("[USER]").excludes(&atom("[US")));
}

/// **A detached request frame frames requests by a key both state**, and only a frame matched by event name alone
/// can: its key is read from the record it was read from, with no span to qualify it. Each refusal says the
/// declaration cannot mean what it says; the attributes the keys are read from are named; and the stored form is the
/// project's and the trace's, never zero.
#[test]
fn a_frame_carrier_frames_requests_by_a_key_both_state() {
    let compiled = |carriers: serde_json::Value| {
        let asset = serde_json::json!({"id": "probe", "doc": "d", "carriers": carriers});
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                serde_json::to_vec(&asset).expect("serialises"),
            )]))
            .expect("the probe assets parse"),
        )
    };
    let frame_facts = serde_json::json!({
        "preset": "snapshot", "carrier_is_detached_request_frame": true, "carrier_holds_span_input": true
    });
    let by_key = serde_json::json!({"frame": "attr:probe.digest", "request": "attr:probe.digest"});
    let frame = |id: &str,
                 matched: serde_json::Value,
                 facts: serde_json::Value,
                 frames: serde_json::Value| {
        serde_json::json!({"id": id, "doc": "d", "match": matched, "facts": facts, "frames_requests": frames})
    };

    let plan = compiled(serde_json::json!([frame(
        "probe.frame",
        serde_json::json!({"event": "probe.instruction"}),
        frame_facts.clone(),
        by_key.clone()
    )]))
    .expect("an event frame keyed by an attribute both state");
    let frames = plan.frames();
    assert!(!frames.is_empty());
    assert_eq!(frames.request_attribute(), Some("probe.digest"));
    assert_eq!(
        frames.frame_attribute("probe.instruction"),
        Some("probe.digest")
    );
    assert_eq!(
        frames.frame_attribute("probe.other"),
        None,
        "a carrier that frames nothing states no frame key"
    );
    assert!(
        compiled(serde_json::json!([]))
            .expect("no carriers")
            .frames()
            .is_empty(),
        "nothing declared frames nothing"
    );
    // The stored form is the project's and the trace's: one key in two traces, or in one trace id of two
    // projects, is two keys; one key in one trace of one project is one; a record outside a trace or a project,
    // or one stating a blank key, frames nothing.
    let stored = |project: &str, trace: &str, key: &str| {
        crate::rules::carrier_rules::RequestFrames::stored_key(project, trace, key)
    };
    let trace = "0af7651916cd43dd8448eb211c80319c";
    let one = stored("p", trace, "d1").expect("keyed");
    assert_ne!(one, 0, "zero is what a record that frames nothing holds");
    assert_eq!(Some(one), stored("p", trace, "d1"));
    assert_ne!(
        Some(one),
        stored("p", "b7ad6b7169203331b7ad6b7169203331", "d1")
    );
    assert_ne!(
        Some(one),
        stored("q", trace, "d1"),
        "another project's trace id"
    );
    assert_ne!(Some(one), stored("p", trace, "d2"));
    assert_ne!(
        stored("ab", "c", "k"),
        stored("a", "bc", "k"),
        "the parts are delimited"
    );
    assert_eq!(stored("p", "", "d1"), None, "outside a trace");
    assert_eq!(stored("", trace, "d1"), None, "outside a project");
    assert_eq!(stored("p", trace, " "), None, "blank");
    // Never zero, by construction rather than by the odds: the digest zero is stored as one.
    let from_digest = crate::rules::carrier_rules::RequestFrames::key_from_digest;
    assert_eq!(from_digest([0; 16]), 1, "zero is the absence of a key");
    let mut low = [0; 16];
    low[15] = 1;
    assert_eq!(from_digest(low), 1);
    assert_eq!(from_digest([0xff; 16]), u128::MAX);
    let mut high = [0; 16];
    high[0] = 1;
    assert_eq!(
        from_digest(high),
        1 << 120,
        "the digest's bytes, most significant first"
    );

    for (why, carriers) in [
        (
            "a carrier that is not a detached frame",
            serde_json::json!([frame(
                "a",
                serde_json::json!({"event": "probe.a"}),
                serde_json::json!({"preset": "snapshot", "carrier_holds_span_input": true}),
                by_key.clone()
            )]),
        ),
        (
            "a frame read from an attribute",
            serde_json::json!([frame(
                "a",
                serde_json::json!({"attribute": "probe.a"}),
                frame_facts.clone(),
                by_key.clone()
            )]),
        ),
        (
            "a frame qualified by the span",
            serde_json::json!([frame(
                "a",
                serde_json::json!({"event": "probe.a", "observation_type": ["generation"]}),
                frame_facts.clone(),
                by_key.clone()
            )]),
        ),
        (
            "a key that is not an attribute",
            serde_json::json!([frame(
                "a",
                serde_json::json!({"event": "probe.a"}),
                frame_facts.clone(),
                serde_json::json!({"frame": "span_name", "request": "attr:probe.digest"})
            )]),
        ),
        (
            "an empty attribute key",
            serde_json::json!([frame(
                "a",
                serde_json::json!({"event": "probe.a"}),
                frame_facts.clone(),
                serde_json::json!({"frame": "attr:probe.digest", "request": "attr:"})
            )]),
        ),
        (
            "two frames keying requests by different attributes",
            serde_json::json!([
                frame(
                    "a",
                    serde_json::json!({"event": "probe.a"}),
                    frame_facts.clone(),
                    by_key.clone()
                ),
                frame(
                    "b",
                    serde_json::json!({"event": "probe.b"}),
                    frame_facts.clone(),
                    serde_json::json!({"frame": "attr:probe.digest", "request": "attr:probe.other"})
                ),
            ]),
        ),
    ] {
        let refused = compiled(carriers)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {why}"));
        assert!(
            matches!(refused, CompileError::Frames { .. }),
            "wrong refusal for {why}: {refused}"
        );
    }
}
