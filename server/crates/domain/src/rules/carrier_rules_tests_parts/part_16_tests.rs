/// **A predicate cannot be written to say nothing.** `equals: null` read as "no condition" and accepted every
/// value; an empty `one_of` held for nothing and an empty `only_members` bounded nothing, and both read as no
/// condition at all. Each is refused where it is parsed, and `kind: null` says what `equals: null` meant.
#[test]
fn a_predicate_written_to_say_nothing_is_refused() {
    let reading = |predicate: &str| {
        format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{{"id":"a","where":{predicate}}}]}}]}}"#
        )
    };
    for predicate in [
        r#"{"path":"$.x","equals":null}"#,
        r#"{"path":"$.x","one_of":[]}"#,
        r#"{"path":"$.x","none_of":[]}"#,
        r#"{"path":"$","kind":"object","only_members":[]}"#,
    ] {
        assert!(
            ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                reading(predicate).into_bytes(),
            )]))
            .is_err(),
            "{predicate} must be refused"
        );
    }
    assert!(probe_compile(&reading(r#"{"path":"$.x","kind":"null"}"#)).is_ok());
    // A span condition's list too.
    let detect = |list: &str| {
        let body = format!(
            r#"{{"id":"t","detect":[{{"id":"t.d","label":"probe","priority":1,"where":{{"source":"attr:x","one_of":{list}}}}}]}}"#
        );
        ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    assert!(detect(r#"["a"]"#).is_ok(), "the probe parses with a list");
    assert!(
        detect("[]").is_err(),
        "an empty span-condition list must be refused"
    );
}

/// **An explicit `null` default is a value.** It read as no default, so a member a shape requires was left off
/// where the asset said to attach `null`.
#[test]
fn an_explicit_null_default_is_attached() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
            "priority":1,"alternatives":[{"id":"a","where":{"path":"$.role"},
            "wrap":{"role_from":"$.role","content_from":"$.content",
              "attach":[{"from":"absent","as":"name","default":null}]}}]}]}"#,
    )
    .expect("the probe compiles");
    let attrs = probe_attrs(&[("x", r#"{"role":"user","content":"hi"}"#)]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    assert_eq!(emitted.len(), 1);
    assert!(
        emitted[0]
            .value
            .get("name")
            .is_some_and(serde_json::Value::is_null),
        "{}",
        emitted[0].value
    );
}

/// **A prune names a clause of its own rule.** A `taken_by` that matches no clause never recognises anything, so
/// the member is never pruned - a misspelling that reads as a deliberate prune.
#[test]
fn a_prune_taken_by_an_undeclared_clause_is_refused() {
    let walk = |taken_by: &str| {
        format!(
            r#"{{"id":"t","messages":[{{"id":"t.w","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                "priority":1,"walk":{{"max_depth":3,"stop_on":["m"],
                  "prune":[{{"member":"messages","taken_by":"{taken_by}"}}]}},
                "also":[{{"id":"m","where":{{"all":[{{"path":"$.role"}},{{"path":"$.content"}}]}},
                  "wrap":{{"role_from":"$.role","content_from":"$.content"}}}}]}}]}}"#
        )
    };
    assert!(
        probe_compile(&walk("m")).is_ok(),
        "a declared clause compiles"
    );
    assert!(
        probe_compile(&walk("n")).is_err(),
        "an undeclared clause must be refused"
    );
}

/// **A member rule has one id, used once.** Two rules under one id shared one meaning and hid each other's
/// priority; a rule with no id could be named by nothing.
#[test]
fn a_member_rule_id_names_one_rule() {
    use crate::rules::members::MemberCompileError;
    let members = |rules: &str| {
        crate::rules::members::compile(&probe_assets(&format!(
            r#"{{"id":"t","message_members":{rules}}}"#
        )))
    };
    let rule = |id: &str, member: &str| {
        format!(
            r#"{{"id":"{id}","members":["{member}"],"means_message_shaped":true,"holds_tool_calls":true}}"#
        )
    };
    assert!(members(&format!("[{}]", rule("t.a", "x"))).is_ok());
    assert!(matches!(
        members(&format!("[{},{}]", rule("t.a", "x"), rule("t.a", "y"))),
        Err(MemberCompileError::DuplicateRule { .. })
    ));
    assert!(matches!(
        members(&format!("[{}]", rule(" ", "x"))),
        Err(MemberCompileError::UnnamedRule { .. })
    ));
}

/// **A carried member adds to a tool definition; it never replaces what the definition is.** `carry` was copied
/// after the definition was built, so carrying `function` swapped the declared function for whatever the payload
/// held under that name.
#[test]
fn a_carried_member_cannot_replace_the_definition() {
    use crate::rules::tool_shapes::{ToolShapeError, ToolShapePlan};
    let shape = |carry: &str| -> crate::rules::schema::RuleFile {
        serde_json::from_value(serde_json::json!({"id": "t", "tool_shapes": [{
            "id": "t.s", "priority": 1, "where": {"path": "$.name", "kind": "string"},
            "name": "$.name", "carry": serde_json::from_str::<serde_json::Value>(carry).expect("carry"),
        }]}))
        .expect("the probe parses")
    };
    assert!(ToolShapePlan::compile(&[shape(r#"["strict"]"#)]).is_ok());
    for carry in [r#"["function"]"#, r#"["type"]"#, r#"["strict","strict"]"#] {
        assert!(
            matches!(
                ToolShapePlan::compile(&[shape(carry)]),
                Err(ToolShapeError::CarryShadows { .. })
            ),
            "{carry} must be refused"
        );
    }
}

/// **A numeric member keeps the integer it was written as.** Read through `f64`, one past 2^53 became its
/// neighbour and every integer gained a fraction.
#[test]
fn a_numeric_member_keeps_its_integer() {
    use crate::rules::message_rules::reading_member_value as member_value;
    let numeric = ["count".to_string()];
    assert_eq!(
        member_value("count", "9007199254740993", &numeric).to_string(),
        "9007199254740993"
    );
    assert_eq!(member_value("count", "42", &numeric).to_string(), "42");
    assert_eq!(member_value("count", "1.5", &numeric).to_string(), "1.5");
    assert_eq!(
        member_value("count", "many", &numeric),
        serde_json::json!("many")
    );
}

/// **A span condition's member test is validated like every other value predicate**, at the polarity the
/// condition sits at: `non_empty` on a boolean can never hold where it is asserted, and is a condition under a
/// `not`.
#[test]
fn a_span_member_test_is_validated_at_its_polarity() {
    let detect = |condition: &str| {
        super::detect_rules::compile(&probe_assets(&format!(
            r#"{{"id":"t","detect":[{{"id":"t.d","label":"Probe","priority":1,"where":{condition}}}]}}"#
        )))
    };
    let member = r#"{"source":"attr:x","parses":"json","member":{"path":"$.a","kind":"bool","non_empty":true}}"#;
    assert!(detect(member).is_err(), "asserted, it can never hold");
    assert!(
        detect(&format!(r#"{{"not":{member}}}"#)).is_ok(),
        "negated, it is a condition"
    );
}

/// **A literal must be able to be of the kind beside it**, and a raw carrier's text is text. `{kind: object,
/// equals: 1}` and `only_members` beside a string hold for nothing; a `raw_where` is asked of the carrier's raw
/// string, so a condition needing another kind, or a member, could never hold there.
#[test]
fn a_condition_no_value_of_its_kind_could_meet_is_refused() {
    let with_where = |predicate: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{{"id":"a","where":{predicate}}}]}}]}}"#
        ))
    };
    assert!(with_where(r#"{"path":"$.x","kind":"number","equals":1}"#).is_ok());
    assert!(with_where(r#"{"path":"$.x","kind":"object","equals":1}"#).is_err());
    assert!(with_where(r#"{"path":"$.x","kind":"string","only_members":["a"]}"#).is_err());
    let raw = |predicate: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                "priority":1,"raw_where":{predicate},
                "alternatives":[{{"id":"a","where":{{"path":"$.role"}}}}]}}]}}"#
        ))
    };
    assert!(
        raw(r#"{"starts_with":"{"}"#).is_ok(),
        "text is what raw_where asks about"
    );
    assert!(raw(r#"{"kind":"object"}"#).is_err());
    assert!(raw(r#"{"path":"$.role","exists":true}"#).is_err());
}

/// **A branch leaf's sections are checked as a top-level rule's are**, and a sections split needs a route: a leaf
/// routing to a misspelled role folded it to `user`, and no route at all emitted nothing.
#[test]
fn a_section_route_is_checked_wherever_it_is() {
    let sections = |routes: &str, priority: &str| {
        format!(
            r#"{{"id":"t.s","read":{{"attribute":"prompt"}},"parse":"text","emit":"message"{priority},
                "sections":{{"split_on":"\n\n","routes":{routes}}}}}"#
        )
    };
    let top = |routes: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{}]}}"#,
            sections(routes, r#","priority":1"#)
        ))
    };
    let leaf = |routes: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.b","priority":1,"branch_set":{{"primary":[{}]}}}}]}}"#,
            sections(routes, "")
        ))
    };
    let route = |role: &str| format!(r#"[{{"id":"r","role":"{role}"}}]"#);
    assert!(top(&route("system")).is_ok());
    assert!(leaf(&route("system")).is_ok());
    assert!(
        leaf(&route("assisstant")).is_err(),
        "a leaf's misspelled role"
    );
    assert!(top("[]").is_err(), "no route");
}

/// **A fragment no rule names is refused.** Its cases are validated where they are inlined, so an unreferenced
/// one carried whatever it said into the corpus unexamined - an invalid predicate included - while the census
/// counted its syntax as used.
#[test]
fn a_fragment_nothing_references_is_refused() {
    let corpus = |reading: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","fragments":{{"turn":{{"doc":"d","cases":[
                {{"id":"c","where":{{"path":"$.role","kind":"bool","non_empty":true}}}}]}}}},
                "messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message","priority":1,
                "alternatives":[{reading}]}}]}}"#
        ))
    };
    assert!(
        corpus(r#"{"id":"a","where":{"path":"$.role"}}"#).is_err(),
        "an unreferenced fragment, invalid predicate and all, is refused"
    );
    assert!(
        corpus(r#"{"id":"a","select":"$.messages","each":true,"then_fragment":"t.turn"}"#).is_err(),
        "referenced, its invalid predicate is found"
    );
}

/// **The ceilings bound the work scheduled, not only the output.** A carrier of more elements than the
/// per-carrier ceiling emits the ceiling and no more - the rest are never built - and a `sections` split is
/// capped the same way, its last section carrying the remainder. A walk over more nodes than its ceiling visits
/// the first of them in document order and schedules nothing past it.
#[test]
fn a_carrier_past_a_ceiling_yields_the_ceiling_in_document_order() {
    use crate::rules::message_rules::MessageContext;
    use sideseat_core::constants::{RULE_MAX_EMISSIONS_PER_CARRIER, RULE_WALK_MAX_NODES};

    // One array, ceiling + 10 elements.
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
            "priority":1,"alternatives":[{"id":"a","select":"$","each":true,
            "where":{"path":"$.role"}}]}]}"#,
    )
    .expect("the probe compiles");
    let many: Vec<String> = (0..RULE_MAX_EMISSIONS_PER_CARRIER + 10)
        .map(|i| format!(r#"{{"role":"user","content":"m{i}"}}"#))
        .collect();
    let attrs = probe_attrs(&[("x", &format!("[{}]", many.join(",")))]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    assert_eq!(emitted.len(), RULE_MAX_EMISSIONS_PER_CARRIER);
    assert_eq!(
        emitted[0].value["content"], "m0",
        "the first elements, in order"
    );

    // A walk over one object of ceiling + 10 children: the first children are read, the rest never scheduled.
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.w","read":{"attribute":"x"},"parse":"json","emit":"message",
            "priority":1,"walk":{"max_depth":2,"stop_on":["m"]},
            "also":[{"id":"m","where":{"all":[{"path":"$.role"},{"path":"$.content"}]},
            "wrap":{"role_from":"$.role","content_from":"$.content"}}]}]}"#,
    )
    .expect("the walk probe compiles");
    let members: Vec<String> = (0..RULE_WALK_MAX_NODES + 10)
        .map(|i| format!(r#""k{i:05}":{{"role":"user","content":"w{i}"}}"#))
        .collect();
    let attrs = probe_attrs(&[("x", &format!("{{{}}}", members.join(",")))]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    // The root is the first visit, so the ceiling leaves room for one less child than itself - the same answer
    // the unbounded scheduling gave, with the stack held to the ceiling.
    assert_eq!(emitted.len(), RULE_WALK_MAX_NODES - 1);
    assert_eq!(emitted[0].value["content"], "w0");

    // Sections: a text of ceiling + 10 pieces keeps the ceiling, the last section carrying the remainder.
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.s","read":{"attribute":"x"},"parse":"text","emit":"message",
            "priority":1,"sections":{"split_on":"|","routes":[{"id":"r","role":"system"}]}}]}"#,
    )
    .expect("the sections probe compiles");
    let pieces: Vec<String> = (0..RULE_MAX_EMISSIONS_PER_CARRIER + 10)
        .map(|i| format!("s{i}"))
        .collect();
    let attrs = probe_attrs(&[("x", &pieces.join("|"))]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    assert_eq!(emitted.len(), RULE_MAX_EMISSIONS_PER_CARRIER);
    let last = emitted
        .last()
        .and_then(|e| e.value["content"].as_str())
        .expect("the last section");
    assert!(
        last.starts_with("s8191|s8192") && last.ends_with("s8201"),
        "the last section carries the remainder: {last:?}"
    );
}
