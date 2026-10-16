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

/// **A clause id names one clause.** Content-block cases and tool shapes accepted empty or repeated ids with
/// distinct priorities, so a diagnostic or census row naming the id named nothing - refused now, in both
/// sections (`ToolShapeError::AmbiguousId`, `ContentBlockCompileError::AmbiguousId`).
#[test]
fn a_clause_id_names_one_clause_in_every_section() {
    let shapes = |ids: &[&str]| -> Result<_, _> {
        let shapes: Vec<serde_json::Value> = ids
            .iter()
            .enumerate()
            .map(|(at, id)| {
                serde_json::json!({"id": id, "priority": at + 1,
                    "where": {"path": "$.name", "kind": "string"}, "name": "$.name"})
            })
            .collect();
        crate::rules::tool_shapes::ToolShapePlan::compile(&[serde_json::from_value(
            serde_json::json!({"id": "t", "tool_shapes": shapes}),
        )
        .expect("the probe parses")])
    };
    assert!(shapes(&["t.a", "t.b"]).is_ok());
    assert!(shapes(&["t.a", "t.a"]).is_err(), "one id twice");
    assert!(shapes(&[" "]).is_err(), "an empty id");

    let blocks = |ids: &[&str]| {
        let cases: Vec<serde_json::Value> = ids
            .iter()
            .enumerate()
            .map(|(at, id)| {
                serde_json::json!({"id": id, "at": "after_provider_formats", "priority": 100_000 + at,
                    "where": {"path": "$.probe_marker", "kind": "string"},
                    "text": {"text": "$.probe_marker"}})
            })
            .collect();
        crate::rules::content_blocks::ContentBlockPlan::compile(
            &[
                serde_json::from_value(serde_json::json!({"id": "t", "content_blocks": cases}))
                    .expect("the probe parses"),
            ],
            &crate::rules::members::MemberPlan::default(),
        )
    };
    assert!(blocks(&["t.a", "t.b"]).is_ok());
    assert!(blocks(&["t.a", "t.a"]).is_err(), "one id twice");
    assert!(blocks(&[""]).is_err(), "an empty id");
}

/// A path's text with the escapes JSONPath decodes in a quoted member (`\uXXXX`) decoded, so a selector cannot
/// hide a member's name by escaping its letters.
fn decoded_path(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\\u") {
        out.push_str(&rest[..at]);
        let code = rest
            .get(at + 2..at + 6)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok());
        match code.and_then(char::from_u32) {
            Some(c) => {
                out.push(c);
                rest = &rest[at + 6..];
            }
            None => {
                out.push_str("\\u");
                rest = &rest[at + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Where an asset reads inside an object `repr` could not print (`__python_opaque`, from `python_repr`): every
/// path naming it outside a `where` test, and every path selecting below it anywhere. The opaque leaf is kept so
/// nothing about it is lost, never so a rule can mine it for content.
fn reads_inside_python_opaque(
    value: &serde_json::Value,
    in_where: bool,
    at: &str,
    out: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, member) in map {
                reads_inside_python_opaque(
                    member,
                    in_where || key == "where",
                    &format!("{at}.{key}"),
                    out,
                );
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                reads_inside_python_opaque(item, in_where, &format!("{at}[{index}]"), out);
            }
        }
        serde_json::Value::String(raw) if decoded_path(raw).contains("__python_") => {
            let text = &decoded_path(raw);
            // Every `__python_` spelling must name one of the tree's own members in full: a selector escaping
            // its letters (`['__python_\\u006fpaque']`) names none and is flagged as the member it may be.
            const MEMBERS: [&str; 5] = ["constructor", "args", "enum", "value", "opaque"];
            let named: Vec<&str> = text
                .split("__python_")
                .skip(1)
                .map(|rest| {
                    let end = rest
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .unwrap_or(rest.len());
                    &rest[..end]
                })
                .collect();
            let unknown = named.iter().any(|name| !MEMBERS.contains(name));
            // Below the member: any further step after it - however its selector is closed, spaced or joined
            // with others.
            let below = text
                .split("__python_opaque")
                .skip(1)
                .any(|rest| rest.contains(['.', '[']));
            let opaque = named.contains(&"opaque");
            if unknown || below || (opaque && !in_where) {
                out.push(format!("{at} = {text:?}"));
            }
        }
        _ => {}
    }
}

/// No asset reads content from inside an opaque leaf: a `where` may ask whether a value is one, nothing may
/// select it or below it.
#[test]
fn no_asset_reads_inside_an_opaque_python_object() {
    let mut found = Vec::new();
    for (path, bytes) in sideseat_rule_assets::sources() {
        let Ok(asset) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        reads_inside_python_opaque(&asset, false, &path, &mut found);
    }
    assert!(
        found.is_empty(),
        "assets reading an opaque Python object: {found:#?}"
    );
    // The check sees both ways of reading one: selecting it, and selecting below it; a `where` asking whether a
    // value is one passes.
    let probe = serde_json::json!({
        "where": {"path": "$.ctx.__python_opaque", "exists": true},
        "select": "$.ctx.__python_opaque",
        "content_from": "$.f.__python_opaque.text",
        "cases": [{"where": {"path": "$.__python_opaque[0]"}}],
        "escaped": "$.ctx['__python_\\u006fpaque']",
        "prefix_escaped": "$.ctx['\\u005f_python_opaque']",
        "quoted_below": {"where": {"path": "$.ctx['__python_opaque'].text"}},
        "spaced_below": {"where": {"path": "$.ctx['__python_opaque' ] .text"}},
        "union_below": {"where": {"path": "$.ctx['__python_opaque','other'].text"}},
        "constructor": "$.__python_constructor",
        // A decode is a selection like any other: it may not open an opaque leaf, on a rule, on an indexed
        // family's entries, or on an overlay - nor may a reading select below one through a decoded member.
        "decode": {"select": "$.ctx.__python_opaque", "parse": "python_repr"},
        "read": {"decode": {"select": "$.content['__python_opaque']", "parse": "java_tostring"}},
        "overlay": {"decode": {"select": "$.raw.__python_opaque.text", "parse": "json"}},
        "through_decoded": {"also": [{"select": "$.ev.ctx.__python_opaque"}]}
    });
    let mut probed = Vec::new();
    reads_inside_python_opaque(&probe, false, "probe", &mut probed);
    probed.sort();
    assert_eq!(
        probed,
        [
            "probe.cases[0].where.path = \"$.__python_opaque[0]\"",
            "probe.content_from = \"$.f.__python_opaque.text\"",
            "probe.decode.select = \"$.ctx.__python_opaque\"",
            "probe.escaped = \"$.ctx['__python_opaque']\"",
            "probe.overlay.decode.select = \"$.raw.__python_opaque.text\"",
            "probe.prefix_escaped = \"$.ctx['__python_opaque']\"",
            "probe.quoted_below.where.path = \"$.ctx['__python_opaque'].text\"",
            "probe.read.decode.select = \"$.content['__python_opaque']\"",
            "probe.select = \"$.ctx.__python_opaque\"",
            "probe.spaced_below.where.path = \"$.ctx['__python_opaque' ] .text\"",
            "probe.through_decoded.also[0].select = \"$.ev.ctx.__python_opaque\"",
            "probe.union_below.where.path = \"$.ctx['__python_opaque','other'].text\"",
        ]
    );
}

/// **`list_members` gathers an entry's items into one list member**, in index order (`10` after `2`, which a
/// member's name order does not give), counts them toward `require_members` as the member they become, leaves an
/// entry already stating that member as it is, and is refused without an indexed family.
#[test]
fn list_members_gathers_an_entry_s_items_in_index_order() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.in","priority":1,"emit":"message",
            "read":{"indexed_family":"fam","entry_member":"message",
              "list_members":{"prefix":"item.","as_member":"content"}},
            "require_members":{"all_of":[{"name":"role"}],"any_of":[{"name":"content"}]}}]}"#,
    )
    .expect("a gathering read compiles");
    let mut pairs: Vec<(String, String)> = vec![("fam.0.message.role".into(), "function".into())];
    for index in [10usize, 2, 0, 1] {
        pairs.push((format!("fam.0.item.{index}"), format!("{{\"n\": {index}}}")));
    }
    pairs.push(("fam.1.message.role".into(), "user".into()));
    pairs.push(("fam.1.message.content".into(), "hello".into()));
    pairs.push(("fam.1.item.0".into(), "{\"n\": 0}".into()));
    let borrowed: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let attrs = probe_attrs(&borrowed);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let entries: Vec<serde_json::Value> = plan.run(&ctx).into_iter().map(|e| e.value).collect();
    assert_eq!(entries.len(), 2, "{entries:?}");
    let gathered: Vec<i64> = entries[0]["content"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| item["n"].as_i64().expect("a number"))
        .collect();
    assert_eq!(gathered, [0, 1, 2, 10]);
    // An entry already stating the member keeps both as they are.
    assert_eq!(entries[1]["content"], serde_json::json!("hello"));
    assert_eq!(entries[1]["item.0"], serde_json::json!({"n": 0}));

    let refused = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.in","priority":1,"emit":"message",
            "read":{"attribute":"x","list_members":{"prefix":"item.","as_member":"content"}}}]}"#,
    );
    assert!(
        refused.is_err(),
        "list_members without an indexed family compiled"
    );
    let refused = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.in","priority":1,"emit":"message",
            "read":{"indexed_family":"fam","list_members":{"prefix":"item","as_member":"content"}}}]}"#,
    );
    assert!(refused.is_err(), "a prefix without its `.` compiled");
}

/// Only an item a `list_members` read gathers admits its entry - `<prefix>` and a canonical index - and each
/// gathered object that declares no type is given the rule's `item_type`.
#[test]
fn list_members_admits_only_what_it_gathers_and_types_its_items() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.in","priority":1,"emit":"message",
            "read":{"indexed_family":"fam","entry_member":"message",
              "list_members":{"prefix":"item.","as_member":"content","item_type":"tool_result"}},
            "require_members":{"all_of":[{"name":"role"}],"any_of":[{"name":"content"}]}}]}"#,
    )
    .expect("a gathering read compiles");
    let run = |pairs: &[(&str, &str)]| {
        let attrs = probe_attrs(pairs);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .into_iter()
            .map(|e| e.value)
            .collect::<Vec<_>>()
    };
    assert!(
        run(&[
            ("fam.0.message.role", "user"),
            ("fam.0.item.01", "{\"n\": 1}")
        ])
        .is_empty()
    );
    assert!(run(&[("fam.0.message.role", "user"), ("fam.0.item.0.text", "x")]).is_empty());
    let typed = run(&[
        ("fam.0.message.role", "function"),
        ("fam.0.item.0", "{\"call_id\": \"c\"}"),
        ("fam.0.item.1", "{\"type\": \"text\", \"text\": \"t\"}"),
    ]);
    assert_eq!(
        typed[0]["content"][0]["type"],
        serde_json::json!("tool_result")
    );
    assert_eq!(typed[0]["content"][1]["type"], serde_json::json!("text"));
}
